use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum GeneratedCommandMetadataError {
    /// OpenAI failed to generate a parsable response.
    BadCommand,
    /// Request to OpenAI failed
    AiProviderError,
    /// User is over rate limit.
    RateLimited,
    Other,
}

impl GeneratedCommandMetadataError {
    pub fn user_facing_message(&self) -> String {
        match self {
            Self::BadCommand => crate::t!("workflow-ai-assist-error-bad-command"),
            Self::AiProviderError => crate::t!("workflow-ai-assist-error-generic"),
            Self::RateLimited => crate::t!("workflow-ai-assist-error-rate-limited"),
            Self::Other => crate::t!("workflow-ai-assist-error-generic"),
        }
    }
}
