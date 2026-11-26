//! End-to-end bridge test: real tokio runtime thread, fake driver and in-memory adapters.
#![allow(clippy::expect_used)] // test helpers

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use application::ports::SystemClock;
use application::registry::DriverRegistry;
use application::usecases::{ExecuteOptions, Ports, UseCases};
use domain::{
    Catalog, ConnectionProfile, DriverKind, QueryResult, ResultColumn, Schema, SchemaTree, Table,
    TableKind, Value,
};
use test_support::{FakeDriver, InMemoryConnections, InMemoryHistory, InMemorySecrets};
use ui_egui::{Backend, Command, Event, SessionId, TabId};

fn setup() -> (Backend, Receiver<Event>, FakeDriver) {
    let schema = SchemaTree {
        catalogs: vec![Catalog {
            name: "db".into(),
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![Table::new("users", TableKind::Table)],
            }],
        }],
    };
    let result = QueryResult {
        columns: vec![ResultColumn {
            name: "n".into(),
            data_type: domain::DataType::new("int"),
        }],
        rows: vec![vec![Value::Int(1)]],
        ..QueryResult::empty()
    };
    let driver = FakeDriver::new("fake")
        .with_schema(schema)
        .with_result(result);
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(driver.clone()));
    let use_cases = UseCases::new(
        Ports {
            connections: Arc::new(InMemoryConnections::default()),
            history: Arc::new(InMemoryHistory::default()),
            secrets: Arc::new(InMemorySecrets::default()),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
        },
        ExecuteOptions::default(),
    );
    let (backend, events) = Backend::spawn(Arc::new(use_cases), || {});
    (backend, events, driver)
}

fn next(events: &Receiver<Event>) -> Event {
    events
        .recv_timeout(Duration::from_secs(5))
        .expect("event within 5s")
}

#[test]
fn connect_browse_query_history_disconnect() {
    let (backend, events, driver) = setup();
    let profile = ConnectionProfile::new("fake", DriverKind::new("fake"), "localhost");
    let id = profile.id;

    backend.send(Command::SaveConnection {
        profile,
        password: None,
    });
    assert!(matches!(next(&events), Event::Saved(Ok(_))));
    backend.send(Command::LoadConnections);
    assert!(matches!(next(&events), Event::Connections(Ok(list)) if list.len() == 1));

    backend.send(Command::Connect(id));
    let Event::Connected {
        result: Ok(session),
        ..
    } = next(&events)
    else {
        panic!("expected a session")
    };

    backend.send(Command::LoadSchema(session));
    assert!(
        matches!(next(&events), Event::Schema { result: Ok(t), .. } if t.tables().count() == 1)
    );

    backend.send(Command::Execute {
        tab: TabId(1),
        session,
        connection: id,
        text: "select 1".into(),
    });
    assert!(
        matches!(next(&events), Event::QueryFinished { result: Ok(r), .. } if r.rows.len() == 1)
    );
    assert_eq!(driver.executed(), vec!["select 1".to_string()]);

    backend.send(Command::LoadHistory(id));
    assert!(matches!(next(&events), Event::History { result: Ok(h), .. }
        if h.len() == 1 && h[0].text == "select 1" && h[0].succeeded));

    backend.send(Command::Disconnect(session));
    assert!(matches!(next(&events), Event::Disconnected(s) if s == session));

    // After disconnecting, the session is gone and queries fail cleanly.
    backend.send(Command::Execute {
        tab: TabId(1),
        session,
        connection: id,
        text: "select 1".into(),
    });
    assert!(matches!(
        next(&events),
        Event::QueryFinished { result: Err(_), .. }
    ));
}

#[test]
fn connecting_to_an_unknown_connection_reports_an_error() {
    let (backend, events, _) = setup();
    backend.send(Command::Connect(domain::ConnectionId::new()));
    assert!(matches!(
        next(&events),
        Event::Connected { result: Err(_), .. }
    ));
    // Commands for sessions that never existed are answered, not ignored or panicking.
    backend.send(Command::LoadSchema(SessionId(99)));
    assert!(matches!(
        next(&events),
        Event::Schema { result: Err(_), .. }
    ));
}

#[test]
fn notify_is_called_for_every_event() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let counter = Arc::new(AtomicUsize::new(0));
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(FakeDriver::new("fake")));
    let use_cases = UseCases::new(
        Ports {
            connections: Arc::new(InMemoryConnections::default()),
            history: Arc::new(InMemoryHistory::default()),
            secrets: Arc::new(InMemorySecrets::default()),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
        },
        ExecuteOptions::default(),
    );
    let c = counter.clone();
    let (backend, events) = Backend::spawn(Arc::new(use_cases), move || {
        c.fetch_add(1, Ordering::SeqCst);
    });
    backend.send(Command::LoadConnections);
    assert!(matches!(next(&events), Event::Connections(Ok(_))));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
