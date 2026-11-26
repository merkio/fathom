//! Connection management: CRUD, test and session lifecycle.
//!
//! Passwords go to the `SecretStore` only. Profiles carry a `SecretRef`, never the secret.

use std::sync::Arc;

use domain::{ConnectionId, ConnectionProfile, SecretRef};
use secrecy::SecretString;

use crate::error::UseCaseError;
use crate::ports::{ConnectionRepository, DbSession, SecretStore, StorageError};
use crate::registry::DriverRegistry;

pub struct SaveConnection {
    repo: Arc<dyn ConnectionRepository>,
    secrets: Arc<dyn SecretStore>,
}

impl SaveConnection {
    pub fn new(repo: Arc<dyn ConnectionRepository>, secrets: Arc<dyn SecretStore>) -> Self {
        Self { repo, secrets }
    }

    /// Validates and stores the profile. A `Some` password is written to the secret store and
    /// the profile gets its `secret_ref`; `None` leaves any existing secret untouched.
    pub async fn execute(
        &self,
        mut profile: ConnectionProfile,
        password: Option<SecretString>,
    ) -> Result<ConnectionProfile, UseCaseError> {
        profile.validate()?;
        if let Some(password) = password {
            let key = SecretRef::for_connection(profile.id);
            self.secrets.set(&key, password).await?;
            profile.secret_ref = Some(key);
        }
        self.repo.save(profile.clone()).await?;
        Ok(profile)
    }
}

pub struct ListConnections {
    repo: Arc<dyn ConnectionRepository>,
}

impl ListConnections {
    pub fn new(repo: Arc<dyn ConnectionRepository>) -> Self {
        Self { repo }
    }

    /// Profiles sorted by name (case-insensitive) for stable display.
    pub async fn execute(&self) -> Result<Vec<ConnectionProfile>, UseCaseError> {
        let mut profiles = self.repo.list().await?;
        profiles.sort_by_key(|p| p.name.to_lowercase());
        Ok(profiles)
    }
}

pub struct DeleteConnection {
    repo: Arc<dyn ConnectionRepository>,
    secrets: Arc<dyn SecretStore>,
}

impl DeleteConnection {
    pub fn new(repo: Arc<dyn ConnectionRepository>, secrets: Arc<dyn SecretStore>) -> Self {
        Self { repo, secrets }
    }

    /// Deletes the profile and its stored secret. Deleting an unknown id is not an error.
    pub async fn execute(&self, id: ConnectionId) -> Result<(), UseCaseError> {
        match self.repo.get(id).await {
            Ok(profile) => {
                if let Some(key) = &profile.secret_ref {
                    self.secrets.delete(key).await?;
                }
            }
            Err(StorageError::NotFound) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        self.repo.delete(id).await?;
        Ok(())
    }
}

async fn connect(
    drivers: &DriverRegistry,
    secrets: &dyn SecretStore,
    profile: &ConnectionProfile,
    password: Option<SecretString>,
) -> Result<Box<dyn DbSession>, UseCaseError> {
    let driver = drivers
        .get(&profile.driver_kind)
        .ok_or_else(|| UseCaseError::UnknownDriver(profile.driver_kind.clone()))?;
    let password = match (password, &profile.secret_ref) {
        (Some(p), _) => Some(p),
        (None, Some(key)) => secrets.get(key).await?,
        (None, None) => None,
    };
    Ok(driver.connect(profile, password).await?)
}

pub struct TestConnection {
    drivers: Arc<DriverRegistry>,
    secrets: Arc<dyn SecretStore>,
}

impl TestConnection {
    pub fn new(drivers: Arc<DriverRegistry>, secrets: Arc<dyn SecretStore>) -> Self {
        Self { drivers, secrets }
    }

    /// Connects and immediately closes. `password` overrides the stored secret, so unsaved
    /// profiles can be tested from the editor form.
    pub async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<SecretString>,
    ) -> Result<(), UseCaseError> {
        profile.validate()?;
        let session = connect(&self.drivers, self.secrets.as_ref(), profile, password).await?;
        session.close().await;
        Ok(())
    }
}

pub struct OpenSession {
    repo: Arc<dyn ConnectionRepository>,
    drivers: Arc<DriverRegistry>,
    secrets: Arc<dyn SecretStore>,
}

impl OpenSession {
    pub fn new(
        repo: Arc<dyn ConnectionRepository>,
        drivers: Arc<DriverRegistry>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            repo,
            drivers,
            secrets,
        }
    }

    pub async fn execute(&self, id: ConnectionId) -> Result<Box<dyn DbSession>, UseCaseError> {
        Ok(self.execute_with_profile(id).await?.1)
    }

    /// Opens the session and also returns the profile it was opened from, so callers can apply
    /// per-connection settings such as read-only mode.
    pub async fn execute_with_profile(
        &self,
        id: ConnectionId,
    ) -> Result<(ConnectionProfile, Box<dyn DbSession>), UseCaseError> {
        let profile = match self.repo.get(id).await {
            Ok(p) => p,
            Err(StorageError::NotFound) => return Err(UseCaseError::ConnectionNotFound(id)),
            Err(e) => return Err(e.into()),
        };
        let session = connect(&self.drivers, self.secrets.as_ref(), &profile, None).await?;
        Ok((profile, session))
    }
}

#[derive(Default)]
pub struct CloseSession;

impl CloseSession {
    pub async fn execute(&self, session: Box<dyn DbSession>) {
        session.close().await;
    }
}
