//! Pluggable completion backends.
//!
//! Extractors are written against the [`LlmBackend`] trait so they don't care
//! whether the text comes from the in-process `llms` crate, a subprocess agent
//! CLI (glmcc / minimaxcc / mimocc), or a test mock.

use async_trait::async_trait;

pub mod agent_cli;
#[cfg(feature = "llms-backend")]
pub mod llms_backend;
pub mod mock;

pub use agent_cli::{AgentCli, AgentCliBackend};
#[cfg(feature = "llms-backend")]
pub use llms_backend::LlmsBackend;
pub use mock::MockBackend;

/// Options for a single completion call.
#[derive(Debug, Clone)]
pub struct CompletionOptions {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl Default for CompletionOptions {
    fn default() -> Self {
        CompletionOptions { model: "qwen-max".into(), temperature: 0.3, max_tokens: 4000 }
    }
}

/// A single chat message.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Message { role: role.into(), content: content.into() }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Message::new("system", content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Message::new("user", content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Message::new("assistant", content)
    }
}

/// A completion backend: turns a message list into assistant text.
#[async_trait]
pub trait LlmBackend: Send + Sync {
    async fn complete(
        &self,
        messages: &[Message],
        options: &CompletionOptions,
    ) -> anyhow::Result<String>;

    /// Convenience for single-prompt calls.
    async fn complete_prompt(
        &self,
        prompt: &str,
        options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        self.complete(&[Message::user(prompt)], options).await
    }
}
