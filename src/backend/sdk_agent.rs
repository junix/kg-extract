//! Completion backend that drives the Claude Code CLI through the structured
//! `claude-agent-sdk-rs` stream-json protocol, rather than a raw `<bin> -p`
//! subprocess that returns plain stdout text.
//!
//! Same model endpoints — the provider environment is injected so the SDK's
//! bundled/`PATH` `claude` binary talks to MiniMax / GLM / MiMo (exactly the
//! `ANTHROPIC_*` vars the `minimaxcc` / `glmcc` / `mimocc` wrapper scripts set).
//! The difference is the wire: we get back parsed `Message`s, so assistant text
//! (and, in future, tool-use blocks) is read cleanly instead of scraped from
//! stdout, and a long-lived `ClaudeSdkClient` could later give real multi-turn.

use std::collections::BTreeMap;

use super::{ChatSession, CompletionOptions, LlmBackend, Message};
use async_trait::async_trait;
use claude_agent_sdk_rs::types::SystemPrompt;
use claude_agent_sdk_rs::{
    query_collect, ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, Message as SdkMessage,
};
use futures::StreamExt;

/// SDK-driven agent backend. Carries the provider env that points the SDK's
/// `claude` at the chosen vendor endpoint.
pub struct SdkAgentBackend {
    provider_env: BTreeMap<String, String>,
    agent: String,
}

/// Build the `ANTHROPIC_*` provider env for a known agent name (`minimaxcc` /
/// `glmcc` / `mimocc`), mirroring the wrapper scripts so the SDK's `claude` talks
/// to the right vendor endpoint. The auth token is read from the same
/// environment variable each wrapper uses and must already be set. Returns the
/// normalised (lowercased) agent name alongside the env.
pub(crate) fn provider_env(name: &str) -> anyhow::Result<(String, BTreeMap<String, String>)> {
    let n = name.trim().to_lowercase();
    let (key_var, base_url, model) = match n.as_str() {
        "minimaxcc" | "minimax" => (
            "MINIMAX_API_KEY",
            "https://api.minimaxi.com/anthropic",
            "MiniMax-M3-highspeed",
        ),
        "glmcc" | "glm" => (
            "GLM_API_KEY",
            "https://open.bigmodel.cn/api/anthropic",
            "glm-5.1",
        ),
        "mimocc" | "mimo" => (
            "MIMO_API_KEY",
            "https://token-plan-cn.xiaomimimo.com/anthropic",
            "mimo-v2.5-pro",
        ),
        other => {
            anyhow::bail!(
                "unknown sdk-agent provider: {other} (expected minimaxcc / glmcc / mimocc)"
            )
        }
    };
    let token = std::env::var(key_var)
        .map_err(|_| anyhow::anyhow!("sdk-agent {n}: environment variable {key_var} is not set"))?;

    let mut env = BTreeMap::new();
    env.insert("ANTHROPIC_AUTH_TOKEN".into(), token);
    env.insert("ANTHROPIC_BASE_URL".into(), base_url.into());
    env.insert("ANTHROPIC_MODEL".into(), model.into());
    env.insert("ANTHROPIC_SMALL_FAST_MODEL".into(), model.into());
    env.insert("API_TIMEOUT_MS".into(), "3000000".into());
    env.insert(
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
        "1".into(),
    );

    Ok((n, env))
}

impl SdkAgentBackend {
    /// Build the backend for a known agent name (`minimaxcc` / `glmcc` /
    /// `mimocc`). See [`provider_env`].
    pub fn for_agent(name: &str) -> anyhow::Result<Self> {
        let (agent, provider_env) = provider_env(name)?;
        Ok(SdkAgentBackend {
            provider_env,
            agent,
        })
    }
}

#[async_trait]
impl LlmBackend for SdkAgentBackend {
    async fn complete(
        &self,
        messages: &[Message],
        _options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        // Flatten the conversation into one prompt — the same mapping
        // the subprocess agent path uses — so behaviour matches it
        // except for the transport. (The system block is folded into the prompt
        // by `flatten_prompt`.)
        let prompt = super::flatten_prompt(messages);

        let mut opts = ClaudeAgentOptions::default();
        opts.env.extend(self.provider_env.clone());
        // Single model response, like `<bin> -p` for a pure extraction prompt.
        opts.max_turns = Some(1);

        let replies = query_collect(prompt, opts)
            .await
            .map_err(|e| anyhow::anyhow!("sdk-agent {} query failed: {e}", self.agent))?;

        let mut out = String::new();
        for m in &replies {
            if let SdkMessage::Assistant(a) = m {
                for block in &a.content {
                    if let ContentBlock::Text(t) = block {
                        out.push_str(&t.text);
                    }
                }
            }
        }
        Ok(out.trim().to_string())
    }

    async fn open_session(
        &self,
        system: Option<String>,
        _options: &CompletionOptions,
    ) -> anyhow::Result<Option<Box<dyn ChatSession>>> {
        let mut opts = ClaudeAgentOptions::default();
        opts.env.extend(self.provider_env.clone());
        if let Some(s) = system {
            opts.system_prompt = Some(SystemPrompt::Text(s));
        }
        // No `max_turns = 1` here — this is the genuinely multi-turn path.
        let client = ClaudeSdkClient::new(opts);
        Ok(Some(Box::new(SdkChatSession {
            client,
            started: false,
            agent: self.agent.clone(),
        })))
    }
}

/// Native multi-turn session over a long-lived [`ClaudeSdkClient`]: one
/// connection, follow-up turns via `query_default`, context retained by the CLI
/// so we never re-send the history.
struct SdkChatSession {
    client: ClaudeSdkClient,
    started: bool,
    agent: String,
}

#[async_trait]
impl ChatSession for SdkChatSession {
    async fn send(&mut self, prompt: &str) -> anyhow::Result<String> {
        if !self.started {
            self.client
                .connect(None)
                .await
                .map_err(|e| anyhow::anyhow!("sdk-agent {} connect failed: {e}", self.agent))?;
            self.started = true;
        }
        self.client
            .query_default(prompt.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("sdk-agent {} query failed: {e}", self.agent))?;

        // `receive_response` yields this turn's messages until the Result marker.
        let mut stream = self
            .client
            .receive_response()
            .map_err(|e| anyhow::anyhow!("sdk-agent {} receive failed: {e}", self.agent))?;
        let mut out = String::new();
        while let Some(msg) = stream.next().await {
            let msg =
                msg.map_err(|e| anyhow::anyhow!("sdk-agent {} stream error: {e}", self.agent))?;
            if let SdkMessage::Assistant(a) = msg {
                for block in &a.content {
                    if let ContentBlock::Text(t) = block {
                        out.push_str(&t.text);
                    }
                }
            }
        }
        Ok(out.trim().to_string())
    }

    async fn finish(&mut self) -> anyhow::Result<()> {
        if self.started {
            self.client
                .disconnect()
                .await
                .map_err(|e| anyhow::anyhow!("sdk-agent {} disconnect failed: {e}", self.agent))?;
        }
        Ok(())
    }
}
