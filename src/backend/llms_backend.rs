//! In-process completion backend backed by the `llms` crate.
//!
//! Resolves a model string through the `llms` registry and dispatches to the
//! right provider (OpenAI-compatible, Ollama, Anthropic, …). This is the
//! backend used by SimpleExtractor (qwen-max), SchemaJsonExtractor (open/fixed
//! modes), and the tool-calling `ToolCallExtractor`.

use super::{CompletionOptions, LlmBackend, Message, ToolChatResponse, ToolInvocation, ToolSpec};
use async_trait::async_trait;
use llms::{
    from_resolved_with_options, resolve_model, ChatMessage, ChatOptions, FunctionDef,
    ToolDefinition,
};

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

fn to_chat_message(m: &Message) -> ChatMessage {
    if let Some(id) = &m.tool_call_id {
        return ChatMessage::tool_result(id.clone(), m.content.clone());
    }
    if let Some(calls) = &m.tool_calls {
        return ChatMessage::assistant_with_tool_calls(Some(m.content.clone()), calls.clone());
    }
    ChatMessage::new(m.role.clone(), m.content.clone())
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
        let chat_messages: Vec<ChatMessage> = messages.iter().map(to_chat_message).collect();
        llm.chat(&chat_messages, None).await
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn complete_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        options: &CompletionOptions,
    ) -> anyhow::Result<ToolChatResponse> {
        let resolved = resolve_model(&options.model, None)?;
        let tool_defs: Vec<ToolDefinition> = tools
            .iter()
            .map(|t| ToolDefinition {
                r#type: "function".into(),
                function: FunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect();
        let chat_opts = ChatOptions {
            max_tokens: Some(options.max_tokens),
            temperature: Some(options.temperature),
            tools: Some(tool_defs),
            ..Default::default()
        };
        let llm = from_resolved_with_options(resolved, Some(chat_opts))?;
        let chat_messages: Vec<ChatMessage> = messages.iter().map(to_chat_message).collect();
        let resp = llm.chat_with_tools(&chat_messages, None).await?;

        let tool_calls = resp
            .tool_calls
            .into_iter()
            .map(|tc| {
                let arguments = serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::Null);
                ToolInvocation {
                    id: tc.id,
                    name: tc.function.name,
                    arguments,
                }
            })
            .collect();
        Ok(ToolChatResponse {
            content: resp.content,
            tool_calls,
        })
    }
}
