//! AI configuration: which provider and model to use, API keys, and model discovery.
//!
//! Keys go to the `SecretStore` only. They are never written to the settings repository, never
//! returned to callers and never logged; callers can only ask whether one is set.

use std::sync::Arc;

use domain::{ModelId, ProviderId};
use secrecy::SecretString;

use crate::error::UseCaseError;
use crate::ports::{ModelInfo, SecretStore, SettingsRepository};
use crate::registry::LlmRegistry;

const PROVIDER_KEY: &str = "ai.provider";
const MODEL_KEY: &str = "ai.model";

/// The user's current choice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiSettings {
    pub provider: Option<ProviderId>,
    pub model: Option<ModelId>,
}

pub struct AiSettingsStore {
    settings: Arc<dyn SettingsRepository>,
}

impl AiSettingsStore {
    pub fn new(settings: Arc<dyn SettingsRepository>) -> Self {
        Self { settings }
    }

    pub async fn load(&self) -> Result<AiSettings, UseCaseError> {
        let get = |v: Option<String>| v.filter(|s| !s.is_empty());
        Ok(AiSettings {
            provider: get(self.settings.get(PROVIDER_KEY).await?).map(ProviderId::new),
            model: get(self.settings.get(MODEL_KEY).await?).map(ModelId::new),
        })
    }

    pub async fn save(&self, settings: &AiSettings) -> Result<(), UseCaseError> {
        let provider = settings.provider.as_ref().map_or("", ProviderId::as_str);
        let model = settings.model.as_ref().map_or("", ModelId::as_str);
        self.settings.set(PROVIDER_KEY, provider).await?;
        self.settings.set(MODEL_KEY, model).await?;
        Ok(())
    }
}

pub struct ApiKeys {
    providers: Arc<LlmRegistry>,
    secrets: Arc<dyn SecretStore>,
}

impl ApiKeys {
    pub fn new(providers: Arc<LlmRegistry>, secrets: Arc<dyn SecretStore>) -> Self {
        Self { providers, secrets }
    }

    /// Stores the key for a provider. Providers that need no key (Ollama) are rejected.
    pub async fn save(&self, provider: &ProviderId, key: SecretString) -> Result<(), UseCaseError> {
        let reference = self
            .providers
            .key_ref(provider)
            .ok_or_else(|| UseCaseError::UnknownProvider(provider.clone()))?;
        self.secrets.set(reference, key).await?;
        Ok(())
    }

    pub async fn clear(&self, provider: &ProviderId) -> Result<(), UseCaseError> {
        if let Some(reference) = self.providers.key_ref(provider) {
            self.secrets.delete(reference).await?;
        }
        Ok(())
    }

    /// Whether a key is stored. `None` when the provider needs no key.
    pub async fn is_set(&self, provider: &ProviderId) -> Result<Option<bool>, UseCaseError> {
        let Some(reference) = self.providers.key_ref(provider) else {
            return Ok(None);
        };
        Ok(Some(self.secrets.get(reference).await?.is_some()))
    }
}

pub struct ListModels {
    providers: Arc<LlmRegistry>,
}

impl ListModels {
    pub fn new(providers: Arc<LlmRegistry>) -> Self {
        Self { providers }
    }

    pub async fn execute(&self, provider: &ProviderId) -> Result<Vec<ModelInfo>, UseCaseError> {
        let provider = self
            .providers
            .get(provider)
            .ok_or_else(|| UseCaseError::UnknownProvider(provider.clone()))?;
        Ok(provider.list_models().await?)
    }
}
