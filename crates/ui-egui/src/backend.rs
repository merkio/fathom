//! Background runtime: receives `Command`s, runs use cases on a tokio runtime and answers with
//! `Event`s. The UI thread only ever does non-blocking channel operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use application::ports::{DbSession, IntrospectScope, LlmChunk};
use application::usecases::{AssistRequest, UseCases};
use domain::{Dialect, QueryRequest};
use futures::StreamExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::bridge::{AiAsk, AiId, Command, Event, SessionId};

const HISTORY_LIMIT: usize = 200;

struct OpenSession {
    session: Box<dyn DbSession>,
    dialect: Dialect,
    /// Per-connection read-only mode: only plain reads may run.
    read_only: bool,
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
            .name("dbiever-runtime".into())
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
                        tokio::spawn(handle(shared.clone(), command));
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

fn text<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
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
                    });
                    sh.sessions.lock().await.insert(id, open);
                    Ok(id)
                }
                Err(e) => Err(text(e)),
            };
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
                Ok(open) => uc
                    .browse_schema
                    .execute(open.session.as_ref(), IntrospectScope::All)
                    .await
                    .map_err(text),
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
                    failure = Some(text(e));
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
    sh.emit(Event::AiFinished { id, result });
}
