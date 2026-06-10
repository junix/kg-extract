//! Deterministic mock backend for tests and offline demos.

use super::{CompletionOptions, LlmBackend, Message, ToolChatResponse, ToolInvocation, ToolSpec};
use async_trait::async_trait;
use std::sync::Mutex;

/// Returns canned responses in sequence; after the list is exhausted returns
/// the last one (or empty). Also supports scripted tool-call rounds for the
/// tool-calling backend path.
pub struct MockBackend {
    responses: Vec<String>,
    /// Scripted tool-call rounds (one Vec per `complete_with_tools` call).
    tool_rounds: Vec<Vec<ToolInvocation>>,
    calls: Mutex<usize>,
    tool_calls_idx: Mutex<usize>,
    pub seen_prompts: Mutex<Vec<String>>,
}

impl MockBackend {
    pub fn new(responses: Vec<String>) -> Self {
        MockBackend {
            responses,
            tool_rounds: Vec::new(),
            calls: Mutex::new(0),
            tool_calls_idx: Mutex::new(0),
            seen_prompts: Mutex::new(Vec::new()),
        }
    }

    pub fn single(response: impl Into<String>) -> Self {
        MockBackend::new(vec![response.into()])
    }

    /// Script tool-call rounds: round *i* is returned by the *i*-th
    /// `complete_with_tools` call (last round repeats once exhausted).
    pub fn with_tool_rounds(mut self, rounds: Vec<Vec<ToolInvocation>>) -> Self {
        self.tool_rounds = rounds;
        self
    }
}

#[async_trait]
impl LlmBackend for MockBackend {
    async fn complete(
        &self,
        messages: &[Message],
        _options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        self.seen_prompts.lock().unwrap().push(
            messages
                .last()
                .map(|m| m.content.clone())
                .unwrap_or_default(),
        );
        let mut n = self.calls.lock().unwrap();
        let idx = (*n).min(self.responses.len().saturating_sub(1));
        *n += 1;
        Ok(self.responses.get(idx).cloned().unwrap_or_default())
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn complete_with_tools(
        &self,
        messages: &[Message],
        _tools: &[ToolSpec],
        _options: &CompletionOptions,
    ) -> anyhow::Result<ToolChatResponse> {
        self.seen_prompts.lock().unwrap().push(
            messages
                .last()
                .map(|m| m.content.clone())
                .unwrap_or_default(),
        );
        let mut i = self.tool_calls_idx.lock().unwrap();
        let idx = *i;
        *i += 1;
        let tool_calls = if self.tool_rounds.is_empty() {
            Vec::new()
        } else {
            self.tool_rounds.get(idx).cloned().unwrap_or_default()
        };
        Ok(ToolChatResponse {
            content: None,
            tool_calls,
        })
    }
}
