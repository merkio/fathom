//! Background runtime: receives `Command`s, runs use cases on a tokio runtime and answers with
//! `Event`s. The UI thread only ever does non-blocking channel operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use application::ports::{DbSession, IntrospectScope};
use application::usecases::UseCases;
use domain::QueryRequest;
use tokio::sync::{Mutex, mpsc};

use crate::bridge::{Command, Event, SessionId};

const HISTORY_LIMIT: usize = 200;

struct OpenSession {
    session: Box<dyn DbSession>,
}

struct Shared {
    use_cases: Arc<UseCases>,
    sessions: Mutex<HashMap<SessionId, Arc<OpenSession>>>,
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
            let result = match uc.open_session.execute(connection).await {
                Ok(session) => {
                    let id = SessionId(sh.next_session.fetch_add(1, Ordering::Relaxed));
                    let open = Arc::new(OpenSession { session });
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
                    .execute(connection, open.session.as_ref(), QueryRequest::sql(sql))
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
    }
}
