//! Completion backend that shells out to a Claude-Code-wrapper agent CLI
//! (`glmcc` / `minimaxcc` / `mimocc`) in headless print mode.
//!
//! Used for YoutuExtractor *agent* mode, where schema-evolving extraction is
//! genuinely agentic and benefits from a full agent loop rather than a single
//! chat call. Default CLI is `minimaxcc`.

use super::{CompletionOptions, LlmBackend, Message};
use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Which agent CLI binary to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentCli {
    #[default]
    Minimaxcc,
    Glmcc,
    Mimocc,
}

impl AgentCli {
    pub fn binary(&self) -> &'static str {
        match self {
            AgentCli::Minimaxcc => "minimaxcc",
            AgentCli::Glmcc => "glmcc",
            AgentCli::Mimocc => "mimocc",
        }
    }

    pub fn parse(s: &str) -> Option<AgentCli> {
        match s.trim().to_lowercase().as_str() {
            "minimaxcc" | "minimax" => Some(AgentCli::Minimaxcc),
            "glmcc" | "glm" => Some(AgentCli::Glmcc),
            "mimocc" | "mimo" => Some(AgentCli::Mimocc),
            _ => None,
        }
    }
}

/// Backend that runs `<bin> -p` (headless print) feeding the prompt on stdin.
pub struct AgentCliBackend {
    pub cli: AgentCli,
    /// Extra args inserted before `-p` (e.g. `--model`, `--permission-mode`).
    pub extra_args: Vec<String>,
}

impl AgentCliBackend {
    pub fn new(cli: AgentCli) -> Self {
        AgentCliBackend { cli, extra_args: Vec::new() }
    }

    pub fn with_args(cli: AgentCli, extra_args: Vec<String>) -> Self {
        AgentCliBackend { cli, extra_args }
    }
}

#[async_trait]
impl LlmBackend for AgentCliBackend {
    async fn complete(
        &self,
        messages: &[Message],
        _options: &CompletionOptions,
    ) -> anyhow::Result<String> {
        // Flatten the message list into a single prompt: system blocks first,
        // then the conversation. Agent CLIs take one prompt on stdin.
        let mut prompt = String::new();
        for m in messages {
            match m.role.as_str() {
                "system" => prompt.push_str(&format!("{}\n\n", m.content)),
                "assistant" => prompt.push_str(&format!("[assistant]\n{}\n\n", m.content)),
                _ => prompt.push_str(&format!("{}\n\n", m.content)),
            }
        }

        let mut cmd = Command::new(self.cli.binary());
        cmd.args(&self.extra_args);
        cmd.arg("-p"); // headless print mode (read prompt from stdin)
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn `{}`: {e}", self.cli.binary()))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.shutdown().await.ok();
        }

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("`{}` exited with {}: {stderr}", self.cli.binary(), output.status);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}
