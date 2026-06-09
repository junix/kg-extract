//! Pluggable completion backends.
//!
//! Extractors are written against the [`LlmBackend`] trait so they don't care
//! whether the text comes from the in-process `llms` crate, a subprocess agent
//! CLI (glmcc / minimaxcc / mimocc via [`AgentCliBackend`], or pi-rs's
//! `pi-agent` via [`PiAgentBackend`]), or a test mock.
//!
//! Backends optionally support **tool / function calling** via
//! [`LlmBackend::complete_with_tools`] (used by the `ToolCallExtractor`); the
//! default implementation reports the backend has no tool support.

use async_trait::async_trait;

pub mod agent_cli;
#[cfg(feature = "llms-backend")]
pub mod llms_backend;
pub mod mock;
pub mod pi_agent;

pub use agent_cli::{AgentCli, AgentCliBackend};
#[cfg(feature = "llms-backend")]
pub use llms_backend::LlmsBackend;
pub use mock::MockBackend;
pub use pi_agent::PiAgentBackend;

/// Flatten a message list into a single prompt string for subprocess agent CLIs
/// that take one prompt on stdin (`AgentCliBackend`, `PiAgentBackend`).
///
/// System blocks come first, then the conversation; assistant turns are tagged
/// so the agent can tell them apart from user input. Both subprocess backends
/// share this so their prompt formatting can't silently diverge.
pub(crate) fn flatten_prompt(messages: &[Message]) -> String {
    let mut prompt = String::new();
    for m in messages {
        match m.role.as_str() {
            "system" => prompt.push_str(&format!("{}\n\n", m.content)),
            "assistant" => prompt.push_str(&format!("[assistant]\n{}\n\n", m.content)),
            _ => prompt.push_str(&format!("{}\n\n", m.content)),
        }
    }
    prompt
}

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

/// A single chat message. Carries optional tool-calling fields (assistant
/// `tool_calls` and `tool_call_id` for tool-result messages), mirroring the
/// `llms` / OpenAI message shape.
#[derive(Debug, Clone, Default)]
pub struct Message {
    pub role: String,
    pub content: String,
    /// Raw OpenAI tool_call objects on an assistant message.
    pub tool_calls: Option<Vec<serde_json::Value>>,
    /// The tool_call id this message answers (role = "tool").
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Message { role: role.into(), content: content.into(), ..Default::default() }
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
    /// Assistant message that issued `tool_calls` (raw OpenAI objects).
    pub fn assistant_with_tool_calls(content: Option<String>, tool_calls: Vec<serde_json::Value>) -> Self {
        Message {
            role: "assistant".into(),
            content: content.unwrap_or_default(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }
    /// A `tool` role message carrying a tool's result.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Message {
            role: "tool".into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// A tool the model may call: name + description + JSON-Schema parameters.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A single tool call requested by the model.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolInvocation {
    /// Rebuild the raw OpenAI tool_call object (for echoing back in an assistant
    /// message during multi-round loops).
    pub fn to_openai_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "type": "function",
            "function": {
                "name": self.name,
                "arguments": serde_json::to_string(&self.arguments).unwrap_or_else(|_| "{}".into()),
            }
        })
    }
}

/// Response from a tool-calling completion.
#[derive(Debug, Clone, Default)]
pub struct ToolChatResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolInvocation>,
}

/// A completion backend: turns a message list into assistant text, and
/// optionally supports tool/function calling.
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

    /// Whether this backend supports tool calling.
    fn supports_tools(&self) -> bool {
        false
    }

    /// Run a completion with tool definitions, returning any tool calls the
    /// model made. Default: unsupported.
    async fn complete_with_tools(
        &self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _options: &CompletionOptions,
    ) -> anyhow::Result<ToolChatResponse> {
        anyhow::bail!("this backend does not support tool calling")
    }
}
