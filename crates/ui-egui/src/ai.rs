//! State of the AI side panel: provider and model choice, the current reply, chat history and
//! the privacy preview. Pure data and logic; the presenter feeds it events and reads it back.

use application::ports::ModelInfo;
use application::usecases::{AiSettings, AssistReply, PromptPreview};
use domain::{ChatMessage, ModelId, ProviderId};

use crate::bridge::{AiId, Event};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiMode {
    Generate,
    Explain,
    Optimize,
    Fix,
    Chat,
}

impl AiMode {
    pub const ALL: [AiMode; 5] = [
        AiMode::Generate,
        AiMode::Explain,
        AiMode::Optimize,
        AiMode::Fix,
        AiMode::Chat,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AiMode::Generate => "Generate",
            AiMode::Explain => "Explain",
            AiMode::Optimize => "Optimize",
            AiMode::Fix => "Fix",
            AiMode::Chat => "Chat",
        }
    }

    /// Whether the user types text for this mode (the others work on the editor's SQL).
    pub fn takes_input(self) -> bool {
        matches!(self, AiMode::Generate | AiMode::Chat)
    }
}

#[derive(Debug)]
pub enum AiRunState {
    Streaming,
    Done(Box<AssistReply>),
    Failed(String),
}

#[derive(Debug)]
pub struct AiRun {
    pub id: AiId,
    pub mode: AiMode,
    /// What the user asked, for display and for the chat history.
    pub prompt: String,
    /// The reply as it streams in.
    pub text: String,
    pub state: AiRunState,
}

#[derive(Debug)]
pub enum ModelsState {
    NotLoaded,
    Loading,
    Ready(Vec<ModelInfo>),
    Failed(String),
}

#[derive(Debug)]
pub struct ProviderState {
    pub id: ProviderId,
    /// Whether this provider authenticates with an API key.
    pub needs_key: bool,
    /// Whether a key is stored (the key itself is never visible to the UI).
    pub key_set: Option<bool>,
    pub models: ModelsState,
}

#[derive(Debug)]
pub struct AiState {
    pub mode: AiMode,
    /// The question or message for `Generate` and `Chat`.
    pub input: String,
    /// Share a few result rows with the next request only. Resets after every request.
    pub include_rows: bool,
    /// Text typed into the API key field; cleared as soon as it is saved.
    pub api_key_input: String,
    pub run: Option<AiRun>,
    pub chat: Vec<ChatMessage>,
    pub preview: Option<PromptPreview>,
    pub providers: Vec<ProviderState>,
    pub settings: AiSettings,
    next_id: u64,
}

impl AiState {
    pub fn new(providers: Vec<(ProviderId, bool)>) -> Self {
        Self {
            mode: AiMode::Generate,
            input: String::new(),
            include_rows: false,
            api_key_input: String::new(),
            run: None,
            chat: Vec::new(),
            preview: None,
            providers: providers
                .into_iter()
                .map(|(id, needs_key)| ProviderState {
                    id,
                    needs_key,
                    key_set: needs_key.then_some(false),
                    models: ModelsState::NotLoaded,
                })
                .collect(),
            settings: AiSettings::default(),
            next_id: 1,
        }
    }

    pub fn next_id(&mut self) -> AiId {
        let id = AiId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn provider(&self, id: &ProviderId) -> Option<&ProviderState> {
        self.providers.iter().find(|p| &p.id == id)
    }

    fn provider_mut(&mut self, id: &ProviderId) -> Option<&mut ProviderState> {
        self.providers.iter_mut().find(|p| &p.id == id)
    }

    pub fn selected_provider(&self) -> Option<&ProviderState> {
        self.settings
            .provider
            .as_ref()
            .and_then(|id| self.provider(id))
    }

    /// The provider and model to use, or why none can be used yet.
    pub fn ready(&self) -> Result<(ProviderId, ModelId), String> {
        let Some(provider) = &self.settings.provider else {
            return Err("Choose an AI provider in the AI settings first.".into());
        };
        let Some(model) = &self.settings.model else {
            return Err("Choose a model in the AI settings first.".into());
        };
        if let Some(state) = self.provider(provider)
            && state.needs_key
            && state.key_set != Some(true)
        {
            return Err(format!(
                "Add an API key for {} in the AI settings first.",
                provider.as_str()
            ));
        }
        Ok((provider.clone(), model.clone()))
    }

    pub fn is_streaming(&self) -> bool {
        matches!(
            self.run,
            Some(AiRun {
                state: AiRunState::Streaming,
                ..
            })
        )
    }

    pub fn begin(&mut self, run: AiRun) {
        self.preview = None;
        self.run = Some(run);
    }

    pub fn clear_chat(&mut self) {
        self.chat.clear();
        if self
            .run
            .as_ref()
            .is_some_and(|r| r.mode == AiMode::Chat && !self.is_streaming())
        {
            self.run = None;
        }
    }

    /// Applies events that only concern the AI panel. Returns follow-up work for the presenter.
    pub fn handle(&mut self, event: Event) -> Vec<Followup> {
        let mut follow = Vec::new();
        match event {
            Event::AiText { id, text } => {
                if let Some(run) = self.run.as_mut().filter(|r| r.id == id)
                    && matches!(run.state, AiRunState::Streaming)
                {
                    run.text.push_str(&text);
                }
            }
            Event::AiFinished { id, result } => {
                let Some(run) = self.run.as_mut().filter(|r| r.id == id) else {
                    return follow;
                };
                match result {
                    Ok(reply) => {
                        if run.mode == AiMode::Chat {
                            self.chat.push(ChatMessage::user(run.prompt.clone()));
                            self.chat.push(ChatMessage::assistant(reply.text.clone()));
                        }
                        run.state = AiRunState::Done(Box::new(reply));
                    }
                    Err(e) => run.state = AiRunState::Failed(e),
                }
            }
            Event::AiPreview(preview) => self.preview = Some(preview),
            Event::AiSettings(Ok(settings)) => {
                if let Some(provider) = settings.provider.clone()
                    && self.provider(&provider).is_some()
                {
                    follow.push(Followup::ListModels(provider));
                    self.settings = settings;
                }
            }
            Event::ApiKeyStatus { provider, set } => {
                if let Some(p) = self.provider_mut(&provider) {
                    p.key_set = set;
                }
            }
            Event::Models { provider, result } => {
                if let Some(p) = self.provider_mut(&provider) {
                    p.models = match result {
                        Ok(models) => ModelsState::Ready(models),
                        Err(e) => ModelsState::Failed(e),
                    };
                }
            }
            _ => {}
        }
        follow
    }

    /// The user picked a provider: forget the model, which belongs to the previous one.
    pub fn select_provider(&mut self, id: ProviderId) -> bool {
        if self.provider(&id).is_none() || self.settings.provider.as_ref() == Some(&id) {
            return false;
        }
        self.settings = AiSettings {
            provider: Some(id.clone()),
            model: None,
        };
        if let Some(p) = self.provider_mut(&id) {
            p.models = ModelsState::Loading;
        }
        true
    }

    pub fn select_model(&mut self, id: ModelId) {
        self.settings.model = Some(id);
    }
}

/// Work the presenter must do after an AI event.
#[derive(Debug, PartialEq, Eq)]
pub enum Followup {
    ListModels(ProviderId),
}
