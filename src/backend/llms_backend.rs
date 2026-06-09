//! In-process completion backend backed by the `llms` crate.
//!
//! Resolves a model string through the `llms` registry and dispatches to the
//! right provider (OpenAI-compatible, Ollama, Anthropic, …). This is the
//! backend used by SimpleExtractor (qwen-max), TriplexExtractor (Ollama
//! `sciphi/triplex`) and YoutuExtractor `noagent` mode.

use super::{CompletionOptions, LlmBackend, Message};
use async_trait::async_trait;
use llms::{from_resolved_with_options, resolve_model, ChatMessage, ChatOptions};

/// Completion backend that calls the `llms` crate.
pub struct LlmsBackend;

impl LlmsBackend {
    pub fn new() -> Self {
        LlmsBackend
    }
}

impl Default for LlmsBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmBackend for LlmsBackend {
    async fn complete(
        &self,
        messages: &[Message],
        options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        let resolved = resolve_model(&options.model, None)?;
        let chat_opts = ChatOptions {
            max_tokens: Some(options.max_tokens),
            temperature: Some(options.temperature),
            ..Default::default()
        };
        let llm = from_resolved_with_options(resolved, Some(chat_opts))?;
        let chat_messages: Vec<ChatMessage> = messages
            .iter()
            .map(|m| ChatMessage::new(m.role.clone(), m.content.clone()))
            .collect();
        llm.chat(&chat_messages, None).await
    }
}
