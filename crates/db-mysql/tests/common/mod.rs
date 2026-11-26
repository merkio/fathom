#![allow(dead_code, clippy::expect_used)]
//! Shared helpers for the integration tests.
//!
//! Point the tests at a server with `DBIEVER_TEST_MYSQL_HOST` (and `_PORT`, `_USER`, `_PASSWORD`,
//! `_DB`), or enable the `containers` feature to start one through Docker
//! (`DBIEVER_TEST_FLAVOR=mariadb` selects MariaDB instead of MySQL).

use domain::{ConnectionProfile, DriverKind};
use secrecy::SecretString;

pub struct Server {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
    #[cfg(feature = "containers")]
    _container: Option<Box<dyn std::any::Any>>,
}

pub async fn server() -> Server {
    if let Ok(host) = std::env::var("DBIEVER_TEST_MYSQL_HOST") {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
        return Server {
            host,
            port: var("DBIEVER_TEST_MYSQL_PORT", "3306")
                .parse()
                .unwrap_or(3306),
            user: var("DBIEVER_TEST_MYSQL_USER", "root"),
            password: var("DBIEVER_TEST_MYSQL_PASSWORD", ""),
            database: var("DBIEVER_TEST_MYSQL_DB", "test"),
            #[cfg(feature = "containers")]
            _container: None,
        };
    }
    #[cfg(feature = "containers")]
    {
        use testcontainers_modules::testcontainers::runners::AsyncRunner;
        let mariadb = std::env::var("DBIEVER_TEST_FLAVOR").is_ok_and(|f| f == "mariadb");
        // Both images allow the empty root password the modules configure; the database is `test`.
        let (container, port): (Box<dyn std::any::Any>, u16) = if mariadb {
            let c = testcontainers_modules::mariadb::Mariadb::default()
                .start()
                .await
                .expect("start mariadb");
            let port = c.get_host_port_ipv4(3306).await.expect("mapped port");
            (Box::new(c), port)
        } else {
            let c = testcontainers_modules::mysql::Mysql::default()
                .start()
                .await
                .expect("start mysql");
            let port = c.get_host_port_ipv4(3306).await.expect("mapped port");
            (Box::new(c), port)
        };
        Server {
            host: "127.0.0.1".into(),
            port,
            user: "root".into(),
            password: String::new(),
            database: "test".into(),
            _container: Some(container),
        }
    }
    #[cfg(not(feature = "containers"))]
    panic!("set DBIEVER_TEST_MYSQL_HOST (and friends) or enable the `containers` feature");
}

/// Profile and password for the server, with TLS off (the test servers have none).
pub fn profile_for(s: &Server) -> (ConnectionProfile, SecretString) {
    let mut profile =
        ConnectionProfile::new("integration", DriverKind::new("mysql"), s.host.clone());
    profile.port = Some(s.port);
    profile.username = s.user.clone();
    profile.database = Some(s.database.clone());
    profile.options.insert("sslmode".into(), "disable".into());
    (profile, SecretString::from(s.password.clone()))
}
