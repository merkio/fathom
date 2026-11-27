//! Background runtime: receives `Command`s, runs use cases on a tokio runtime and answers with
//! `Event`s. The UI thread only ever does non-blocking channel operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use application::error::UseCaseError;
use application::error_text::describe;
use application::ports::{DbSession, IntrospectScope, LlmChunk};
use application::redact::redact;
use application::usecases::{AssistRequest, ExportResult, InlineRequest, UseCases};
use domain::{Dialect, QueryRequest, SchemaTree};
use futures::StreamExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::bridge::{AiAsk, AiId, Command, Event, ExportDone, SessionId};
use crate::export::resolve_path;

const HISTORY_LIMIT: usize = 200;

struct OpenSession {
    session: Box<dyn DbSession>,
    dialect: Dialect,
    /// Per-connection read-only mode: only plain reads may run.
    read_only: bool,
    /// Per-connection override of inline suggestions.
    inline_override: Option<bool>,
    /// The latest loaded schema, so inline requests need not carry it.
    schema: std::sync::Mutex<Option<Arc<SchemaTree>>>,
}

struct Shared {
    use_cases: Arc<UseCases>,
    sessions: Mutex<HashMap<SessionId, Arc<OpenSession>>>,
    /// Running AI requests, each with the channel that stops it.
    ai_stops: Mutex<HashMap<AiId, oneshot::Sender<()>>>,
    next_session: AtomicU64,
    events: Sender<Event>,
    notify: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn emit(&self, event: Event) {
        // The UI may already be gone during shutdown; nothing to do then.
        let _ = self.events.send(event);
        (self.notify)();
    }

    async fn session(&self, id: SessionId) -> Result<Arc<OpenSession>, String> {
        self.sessions
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| "session is closed".to_owned())
    }
}

/// Handle used by the UI to send commands. Dropping it stops the runtime thread.
pub struct Backend {
    commands: mpsc::UnboundedSender<Command>,
}

impl Backend {
    /// Starts the runtime thread. `notify` is called after every event (the UI passes
    /// `ctx.request_repaint`). Returns the handle and the event receiver to poll each frame.
    pub fn spawn(
        use_cases: Arc<UseCases>,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> (Self, Receiver<Event>) {
        let (events, event_rx) = channel();
        let (commands, mut command_rx) = mpsc::unbounded_channel::<Command>();
        let shared = Arc::new(Shared {
            use_cases,
            sessions: Mutex::new(HashMap::new()),
            ai_stops: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            events,
            notify: Box::new(notify),
        });

        std::thread::Builder::new()
            .name("fathom-runtime".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    while let Some(command) = command_rx.recv().await {
                        if command.is_ordered() {
                            // Quick writes run in arrival order, so a later read or write
                            // always sees (or follows) an earlier one.
                            handle(shared.clone(), command).await;
                        } else {
                            tokio::spawn(handle(shared.clone(), command));
                        }
                    }
                });
            })
            .ok();

        (Self { commands }, event_rx)
    }

    /// Non-blocking.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// Message for the user: what happened and what to try, with any secret scrubbed.
fn text(e: UseCaseError) -> String {
    describe(&e).to_text()
}

fn llm_text(e: application::ports::LlmError) -> String {
    text(UseCaseError::Llm(e))
}

async fn handle(sh: Arc<Shared>, command: Command) {
    let uc = &sh.use_cases;
    match command {
        Command::LoadConnections => {
            let result = uc.list_connections.execute().await.map_err(text);
            sh.emit(Event::Connections(result));
        }
        Command::SaveConnection { profile, password } => {
            let result = uc
                .save_connection
                .execute(profile, password)
                .await
                .map_err(text);
            sh.emit(Event::Saved(result));
        }
        Command::DeleteConnection(id) => {
            let result = uc
                .delete_connection
                .execute(id)
                .await
                .map(|()| id)
                .map_err(text);
            sh.emit(Event::Deleted(result));
        }
        Command::TestConnection { profile, password } => {
            let result = uc
                .test_connection
                .execute(&profile, password)
                .await
                .map_err(text);
            sh.emit(Event::TestFinished(result));
        }
        Command::Connect(connection) => {
            let result = match uc.open_session.execute_with_profile(connection).await {
                Ok((profile, session)) => {
                    let id = SessionId(sh.next_session.fetch_add(1, Ordering::Relaxed));
                    let open = Arc::new(OpenSession {
                        session,
                        dialect: uc.dialect_for(&profile.driver_kind),
                        read_only: profile.read_only(),
                        inline_override: profile.inline_ai(),
                        schema: std::sync::Mutex::new(None),
                    });
                    sh.sessions.lock().await.insert(id, open);
                    Ok(id)
                }
                Err(e) => Err(text(e)),
            };
            match &result {
                Ok(session) => {
                    tracing::info!(connection = %connection.0, session = session.0, "connected")
                }
                Err(message) => {
                    tracing::warn!(connection = %connection.0, error = %redact(message), "connect failed")
                }
            }
            sh.emit(Event::Connected { connection, result });
        }
        Command::Disconnect(id) => {
            let removed = sh.sessions.lock().await.remove(&id);
            if let Some(open) = removed {
                match Arc::try_unwrap(open) {
                    Ok(open) => uc.close_session.execute(open.session).await,
                    // Still in use by a running statement: stop it; the connection closes on drop.
                    Err(shared) => {
                        let _ = shared.session.cancel().await;
                    }
                }
            }
            sh.emit(Event::Disconnected(id));
        }
        Command::LoadSchema(id) => {
            let result = match sh.session(id).await {
                Ok(open) => {
                    let loaded = uc
                        .browse_schema
                        .execute(open.session.as_ref(), IntrospectScope::All)
                        .await
                        .map_err(text);
                    if let Ok(tree) = &loaded
                        && let Ok(mut cache) = open.schema.lock()
                    {
                        *cache = Some(Arc::new(tree.clone()));
                    }
                    loaded
                }
                Err(e) => Err(e),
            };
            sh.emit(Event::Schema {
                session: id,
                result,
            });
        }
        Command::Execute {
            tab,
            session,
            connection,
            text: sql,
        } => {
            let started = std::time::Instant::now();
            let result = match sh.session(session).await {
                Ok(open) => uc
                    .execute_query
                    .execute_guarded(
                        connection,
                        open.session.as_ref(),
                        QueryRequest::sql(sql),
                        &open.dialect,
                        open.read_only,
                    )
                    .await
                    .map_err(text),
                Err(e) => Err(e),
            };
            // Never log the SQL text: it can contain sensitive literals.
            match &result {
                Ok(r) => tracing::info!(
                    session = session.0,
                    rows = r.rows.len(),
                    truncated = r.truncated,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "query finished"
                ),
                Err(message) => tracing::warn!(
                    session = session.0,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %redact(message),
                    "query failed"
                ),
            }
            sh.emit(Event::QueryFinished { tab, result });
        }
        Command::Cancel(id) => {
            if let Ok(open) = sh.session(id).await {
                let _ = open.session.cancel().await;
            }
        }
        Command::LoadHistory(connection) => {
            let result = uc
                .query_history
                .recent(connection, HISTORY_LIMIT)
                .await
                .map_err(text);
            sh.emit(Event::History { connection, result });
        }
        Command::AiAsk(ask) => run_ai(&sh, *ask).await,
        Command::AiStop(id) => {
            if let Some(stop) = sh.ai_stops.lock().await.remove(&id) {
                let _ = stop.send(());
            }
        }
        Command::AiPreview(ask) => {
            let req = assist_request(&ask);
            sh.emit(Event::AiPreview(uc.assistant.preview(&req)));
        }
        Command::LoadAiSettings => {
            let result = uc.ai_settings.load().await.map_err(text);
            sh.emit(Event::AiSettings(result));
        }
        Command::SaveAiSettings(settings) => {
            if let Err(e) = uc.ai_settings.save(&settings).await {
                sh.emit(Event::Notice(Err(format!(
                    "Could not save AI settings: {e}"
                ))));
            }
        }
        Command::SaveApiKey { provider, key } => {
            let result = match uc.api_keys.save(&provider, key).await {
                Ok(()) => Ok(format!("API key for {} saved", provider.as_str())),
                Err(e) => Err(format!("Could not save the API key: {e}")),
            };
            sh.emit(Event::Notice(result));
            let set = uc.api_keys.is_set(&provider).await.ok().flatten();
            sh.emit(Event::ApiKeyStatus { provider, set });
        }
        Command::ClearApiKey(provider) => {
            let result = match uc.api_keys.clear(&provider).await {
                Ok(()) => Ok(format!("API key for {} removed", provider.as_str())),
                Err(e) => Err(format!("Could not remove the API key: {e}")),
            };
            sh.emit(Event::Notice(result));
            let set = uc.api_keys.is_set(&provider).await.ok().flatten();
            sh.emit(Event::ApiKeyStatus { provider, set });
        }
        Command::LoadApiKeyStatus(provider) => {
            let set = uc.api_keys.is_set(&provider).await.ok().flatten();
            sh.emit(Event::ApiKeyStatus { provider, set });
        }
        Command::InlineSuggest {
            id,
            session,
            text: editor,
            cursor,
        } => {
            run_inline(&sh, id, session, editor, cursor).await;
        }
        Command::InlineCancel(id) => {
            if let Some(stop) = sh.ai_stops.lock().await.remove(&id) {
                let _ = stop.send(());
            }
        }
        Command::LoadInlineSettings => {
            let result = uc
                .suggest_inline
                .settings()
                .load()
                .await
                .map_err(|e| text(UseCaseError::Storage(e)));
            sh.emit(Event::InlineSettings(result));
        }
        Command::SaveInlineSettings(settings) => {
            if let Err(e) = uc.suggest_inline.settings().save(&settings).await {
                sh.emit(Event::Notice(Err(format!(
                    "Could not save the inline settings: {e}"
                ))));
            }
        }
        Command::LoadVisibility(connection) => {
            let result = uc
                .explorer_visibility
                .load(connection)
                .await
                .map_err(|e| text(UseCaseError::Storage(e)));
            sh.emit(Event::Visibility { connection, result });
        }
        Command::SaveVisibility {
            connection,
            visible,
        } => {
            if let Err(e) = uc.explorer_visibility.save(connection, &visible).await {
                sh.emit(Event::Notice(Err(format!(
                    "Could not save the explorer choice: {e}"
                ))));
            }
        }
        Command::LoadUiPrefs => {
            let result = uc
                .ui_prefs
                .load()
                .await
                .map_err(|e| text(UseCaseError::Storage(e)));
            sh.emit(Event::UiPrefs(result));
        }
        Command::SaveUiPrefs(prefs) => {
            if let Err(e) = uc.ui_prefs.save(&prefs).await {
                sh.emit(Event::Notice(Err(format!(
                    "Could not save the layout settings: {e}"
                ))));
            }
        }
        Command::Export {
            tab,
            session,
            connection,
            text: sql,
            format,
            name,
        } => {
            let result = run_export(&sh, session, connection, sql, format, name).await;
            match &result {
                Ok(done) => tracing::info!(
                    rows = done.rows,
                    truncated = done.truncated,
                    "export finished"
                ),
                Err(message) => tracing::warn!(error = %redact(message), "export failed"),
            }
            sh.emit(Event::Exported { tab, result });
        }
        Command::ListModels(provider) => {
            let result = uc.list_models.execute(&provider).await.map_err(text);
            sh.emit(Event::Models { provider, result });
        }
    }
}

fn assist_request(ask: &AiAsk) -> AssistRequest<'_> {
    AssistRequest {
        provider: ask.provider.clone(),
        model: ask.model.clone(),
        dialect: &ask.dialect,
        schema: &ask.schema,
        task: ask.task.as_task(),
        sample_rows: ask.sample_rows.as_ref(),
        max_tokens: None,
    }
}

/// Streams one AI reply, forwarding text as it arrives. The reply is parsed and classified when
/// the stream ends, or when the user stops it. Nothing here runs SQL.
async fn run_ai(sh: &Arc<Shared>, ask: AiAsk) {
    let uc = &sh.use_cases;
    let id = ask.id;
    // Provider and model only; never the prompt, the schema or any row data.
    tracing::info!(
        provider = ask.provider.as_str(),
        model = ask.model.as_str(),
        "ai request started"
    );
    let req = assist_request(&ask);
    let mut stream = match uc.assistant.stream(&req) {
        Ok(s) => s,
        Err(e) => {
            sh.emit(Event::AiFinished {
                id,
                result: Err(text(e)),
            });
            return;
        }
    };
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    sh.ai_stops.lock().await.insert(id, stop_tx);

    let mut reply = String::new();
    let mut usage = None;
    let mut stopped = false;
    let mut failure = None;
    loop {
        tokio::select! {
            chunk = stream.next() => match chunk {
                Some(Ok(LlmChunk::Text(t))) => {
                    reply.push_str(&t);
                    sh.emit(Event::AiText { id, text: t });
                }
                Some(Ok(LlmChunk::Usage(u))) => usage = Some(u),
                Some(Ok(LlmChunk::Done)) | None => break,
                Some(Err(e)) => {
                    failure = Some(llm_text(e));
                    break;
                }
            },
            _ = &mut stop_rx => {
                stopped = true;
                break;
            }
        }
    }
    // Dropping the stream closes the connection, which cancels the request.
    drop(stream);
    sh.ai_stops.lock().await.remove(&id);

    let result = match failure {
        Some(e) => Err(e),
        None => {
            let mut finished = uc.assistant.finish(&reply, &ask.dialect, usage);
            if stopped {
                let note = "Stopped by you; the reply is incomplete.";
                finished.warning = Some(match finished.warning.take() {
                    Some(w) => format!("{note} {w}"),
                    None => note.to_owned(),
                });
            }
            Ok(finished)
        }
    };
    match &result {
        Ok(reply) => tracing::info!(
            has_sql = reply.sql.is_some(),
            input_tokens = reply.usage.map_or(0, |u| u.input_tokens),
            output_tokens = reply.usage.map_or(0, |u| u.output_tokens),
            "ai request finished"
        ),
        Err(message) => tracing::warn!(error = %redact(message), "ai request failed"),
    }
    sh.emit(Event::AiFinished { id, result });
}

/// Streams one inline completion. Cancelling (a newer keystroke made it stale) drops the stream,
/// which closes the connection. When suggestions are off the use case refuses before any provider
/// is contacted and this reports a quiet "done".
async fn run_inline(sh: &Arc<Shared>, id: AiId, session: SessionId, editor: String, cursor: usize) {
    let uc = &sh.use_cases;
    let Ok(open) = sh.session(session).await else {
        sh.emit(Event::InlineDone { id, error: None });
        return;
    };
    let schema = open.schema.lock().ok().and_then(|g| g.clone());
    let Some(schema) = schema else {
        // No schema yet: nothing useful to ask.
        sh.emit(Event::InlineDone { id, error: None });
        return;
    };
    let request = InlineRequest {
        dialect: &open.dialect,
        schema: &schema,
        text: &editor,
        cursor,
        connection_override: open.inline_override,
    };
    let mut stream = match uc.suggest_inline.start(&request).await {
        Ok(s) => s,
        Err(application::error::UseCaseError::Blocked(_)) => {
            sh.emit(Event::InlineDone { id, error: None });
            return;
        }
        Err(e) => {
            sh.emit(Event::InlineDone {
                id,
                error: Some(text(e)),
            });
            return;
        }
    };
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    sh.ai_stops.lock().await.insert(id, stop_tx);

    let mut failure = None;
    loop {
        tokio::select! {
            item = stream.next() => match item {
                Some(Ok(completion)) => sh.emit(Event::InlineText { id, completion }),
                Some(Err(e)) => {
                    failure = Some(llm_text(e));
                    break;
                }
                None => break,
            },
            _ = &mut stop_rx => break,
        }
    }
    drop(stream);
    sh.ai_stops.lock().await.remove(&id);
    sh.emit(Event::InlineDone { id, error: failure });
}

/// Re-runs the statement (up to the export cap) and writes it to a **new** file: an existing
/// file is never overwritten, and a failed export leaves no partial file behind.
async fn run_export(
    sh: &Arc<Shared>,
    session: SessionId,
    connection: domain::ConnectionId,
    sql: String,
    format: application::export::ExportFormat,
    name: String,
) -> Result<ExportDone, String> {
    use std::io::Write;

    let uc = &sh.use_cases;
    let open = sh.session(session).await?;
    let path = resolve_path(&uc.export.directory, &name, format)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| text(UseCaseError::Io(e.to_string())))?;
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                format!("`{}` already exists. Choose another name; existing files are never overwritten.", path.display())
            }
            _ => text(UseCaseError::Io(e.to_string())),
        })?;
    let mut out = std::io::BufWriter::new(file);

    let summary = ExportResult::new(&uc.execute_query)
        .run(
            connection,
            open.session.as_ref(),
            &sql,
            &open.dialect,
            open.read_only,
            format,
            uc.export.max_rows,
            &mut out,
        )
        .await;
    let finished = summary.and_then(|s| {
        out.flush()
            .map(|()| s)
            .map_err(|e| UseCaseError::Io(e.to_string()))
    });
    match finished {
        Ok(summary) => Ok(ExportDone {
            path,
            rows: summary.rows,
            truncated: summary.truncated,
        }),
        Err(e) => {
            drop(out);
            let _ = std::fs::remove_file(&path);
            Err(text(e))
        }
    }
}
