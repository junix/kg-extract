//! Completion backend that drives pi-rs's [`pi-agent`] one-shot CLI.
//!
//! [`pi-agent`]: https://github.com/junix/pi-rs
//!
//! `pi-agent` is the CLI shipped by the `pi-rs` crate: a one-shot agent driver
//! built on the `@earendil-works/pi-agent-core` contract. Unlike the
//! Claude-Code wrappers behind [`SdkAgentBackend`](super::SdkAgentBackend), it
//! speaks a different invocation contract, so it gets its own backend:
//!
//! - the prompt is read from stdin only when invoked as `-p -` (a bare `-p`
//!   would consume the next token as the prompt);
//! - output is produced **only** with `--stream-json`, as LF-delimited JSON
//!   records on stdout (it errors out otherwise);
//! - the answer is therefore a *stream* of records, not plain text.
//!
//! This backend folds that stream back into a single answer string: it returns
//! the final `{"type":"assistant_message","text":…}` record, falling back to
//! the concatenation of `{"type":"assistant_text_delta","delta":…}` chunks, and
//! surfaces `{"type":"error","message":…}` records (and pre-stream errors on
//! stderr) as a Rust error.
//!
//! Tools are disabled by default (`--no-tools`) and project-local `.omp`
//! discovery is suppressed (`--no-approve`): this backend is used as a plain
//! text-completion provider for the extractors, which parse the model's text
//! output themselves — pi-agent's own file/shell tool loop would only add
//! nondeterminism. The model, base URL, and API-key env var are governed by
//! `pi-agent`'s own flags/defaults; pass them through [`extra_args`].
//!
//! [`extra_args`]: PiAgentBackend::extra_args

use super::{CompletionOptions, LlmBackend, Message};
use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Backend that runs `pi-agent -p - --stream-json --no-tools --no-approve` and
/// parses the JSONL stream back into a single answer string.
pub struct PiAgentBackend {
    /// Binary name or path (default `pi-agent`, resolved on `PATH`).
    pub binary: String,
    /// Extra args inserted before `-p` — typically `--model`, `--base-url`, or
    /// `--api-key-env`. Empty by default.
    pub extra_args: Vec<String>,
}

impl Default for PiAgentBackend {
    fn default() -> Self {
        PiAgentBackend {
            binary: "pi-agent".to_string(),
            extra_args: Vec::new(),
        }
    }
}

impl PiAgentBackend {
    /// A backend driving the `pi-agent` binary on `PATH` with no extra args.
    pub fn new() -> Self {
        PiAgentBackend::default()
    }

    /// A backend with extra args inserted before `-p` (e.g. `--model X`).
    pub fn with_args(extra_args: Vec<String>) -> Self {
        PiAgentBackend {
            binary: "pi-agent".to_string(),
            extra_args,
        }
    }

    /// A backend driving an explicit binary path (used by tests with a fake
    /// `pi-agent`) plus extra args.
    pub fn with_binary(binary: impl Into<String>, extra_args: Vec<String>) -> Self {
        PiAgentBackend {
            binary: binary.into(),
            extra_args,
        }
    }

    /// Whether `name` selects the pi-agent backend (case-insensitive). Used by
    /// the CLI to route `--backend agent --agent pi-agent` here instead of to
    /// [`SdkAgentBackend`](super::SdkAgentBackend).
    pub fn accepts(name: &str) -> bool {
        matches!(
            name.trim().to_lowercase().as_str(),
            "pi-agent" | "pi_agent" | "piagent" | "pi"
        )
    }
}

#[async_trait]
impl LlmBackend for PiAgentBackend {
    async fn complete(
        &self,
        messages: &[Message],
        _options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        let prompt = super::flatten_prompt(messages);

        let mut cmd = Command::new(&self.binary);
        cmd.args(&self.extra_args);
        // `-p -` reads the prompt from stdin; `--stream-json` is required for
        // pi-agent to emit anything; tools/`.omp` discovery off for determinism.
        cmd.args(["-p", "-", "--stream-json", "--no-tools", "--no-approve"]);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn `{}`: {e}", self.binary))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.shutdown().await.ok();
        }

        let output = child.wait_with_output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        // On a non-zero exit, pi-agent may still have emitted an `error` record
        // on stdout (stream already started) — prefer the structured message;
        // otherwise fall back to stderr (pre-stream errors print there).
        if !output.status.success() {
            return match extract_assistant_text(&stdout) {
                Ok(text) => Ok(text),
                Err(parse_err) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let detail = stderr.trim();
                    if detail.is_empty() {
                        anyhow::bail!(
                            "`{}` exited with {}: {parse_err}",
                            self.binary,
                            output.status
                        )
                    }
                    anyhow::bail!("`{}` exited with {}: {detail}", self.binary, output.status)
                }
            };
        }

        extract_assistant_text(&stdout)
    }
}

/// Parse pi-agent's `--stream-json` stdout (LF-delimited JSON records) into the
/// final assistant answer.
///
/// Priority:
///   1. a **non-empty** `{"type":"assistant_message","text":…}` record (covers
///      error-then-recovery within one run);
///   2. an `{"type":"error","message":…}` (or `agent_end` with `ok:false`)
///      record — surfaced as an error. This must outrank an *empty*
///      assistant_message: when a provider call fails mid-stream (e.g. an HTTP
///      404), pi-agent still emits `{"text":"","type":"assistant_message"}` then
///      `agent_end ok:true`, so an empty answer must not mask the real error;
///   3. otherwise the concatenation of `{"type":"assistant_text_delta","delta":…}`;
///   4. an explicit but empty assistant_message with no error → `Ok("")`.
///
/// Lines that aren't JSON are ignored — pi-agent writes diagnostics to stderr,
/// but be lenient about stray stdout noise.
fn extract_assistant_text(stdout: &str) -> anyhow::Result<String> {
    let mut final_text: Option<String> = None;
    let mut deltas = String::new();
    let mut error: Option<String> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match value.get("type").and_then(|t| t.as_str()) {
            Some("assistant_message") => {
                if let Some(text) = value.get("text").and_then(|t| t.as_str()) {
                    final_text = Some(text.to_string());
                }
            }
            Some("assistant_text_delta") => {
                if let Some(delta) = value.get("delta").and_then(|d| d.as_str()) {
                    deltas.push_str(delta);
                }
            }
            Some("error") => {
                let message = value
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                error = Some(message.to_string());
            }
            Some("agent_end")
                if value.get("ok").and_then(|o| o.as_bool()) == Some(false) && error.is_none() =>
            {
                error = Some("pi-agent reported agent_end ok=false".to_string());
            }
            _ => {}
        }
    }

    // A non-empty final answer wins outright (covers error-then-recovery).
    if let Some(text) = final_text.as_deref() {
        if !text.is_empty() {
            return Ok(text.to_string());
        }
    }
    // No usable answer text: surface any error rather than let an empty
    // assistant_message (emitted after a failed provider call) mask it.
    if let Some(message) = error {
        anyhow::bail!("pi-agent error: {message}");
    }
    // Streaming-only output with no final message.
    if !deltas.is_empty() {
        return Ok(deltas);
    }
    // The agent ran and explicitly produced empty output (no error reported).
    if final_text.is_some() {
        return Ok(String::new());
    }
    anyhow::bail!("pi-agent produced no assistant_message in its --stream-json output")
}

#[cfg(test)]
#[path = "pi_agent_test.rs"]
mod tests;
