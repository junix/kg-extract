//! Deterministic mock backend for tests and offline demos.

use super::{CompletionOptions, LlmBackend, Message};
use async_trait::async_trait;
use std::sync::Mutex;

/// Returns canned responses in sequence; after the list is exhausted returns
/// the last one (or empty). Records the prompts it received.
pub struct MockBackend {
    responses: Vec<String>,
    calls: Mutex<usize>,
    pub seen_prompts: Mutex<Vec<String>>,
}

impl MockBackend {
    pub fn new(responses: Vec<String>) -> Self {
        MockBackend { responses, calls: Mutex::new(0), seen_prompts: Mutex::new(Vec::new()) }
    }

    pub fn single(response: impl Into<String>) -> Self {
        MockBackend::new(vec![response.into()])
    }
}

#[async_trait]
impl LlmBackend for MockBackend {
    async fn complete(
        &self,
        messages: &[Message],
        _options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        self.seen_prompts
            .lock()
            .unwrap()
            .push(messages.last().map(|m| m.content.clone()).unwrap_or_default());
        let mut n = self.calls.lock().unwrap();
        let idx = (*n).min(self.responses.len().saturating_sub(1));
        *n += 1;
        Ok(self.responses.get(idx).cloned().unwrap_or_default())
    }
}
