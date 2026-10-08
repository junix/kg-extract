use super::*;

// ---- name routing -------------------------------------------------------

#[test]
fn accepts_recognizes_pi_agent_aliases() {
    for name in [
        "pi-agent",
        "pi_agent",
        "piagent",
        "pi",
        "PI-AGENT",
        "  Pi-Agent  ",
    ] {
        assert!(
            PiAgentBackend::accepts(name),
            "{name:?} should select pi-agent"
        );
    }
    for name in ["minimaxcc", "glmcc", "mimocc", "pip", "", "agent"] {
        assert!(
            !PiAgentBackend::accepts(name),
            "{name:?} should not select pi-agent"
        );
    }
}

// ---- stream-json parsing ------------------------------------------------

#[test]
fn parses_final_assistant_message() {
    let stdout = concat!(
        r#"{"type":"agent_start"}"#,
        "\n",
        r#"{"type":"assistant_text_delta","delta":"par"}"#,
        "\n",
        r#"{"type":"assistant_text_delta","delta":"tial"}"#,
        "\n",
        r#"{"type":"assistant_message","text":"the full answer"}"#,
        "\n",
        r#"{"type":"agent_end","ok":true}"#,
        "\n",
    );
    assert_eq!(extract_assistant_text(stdout).unwrap(), "the full answer");
}

#[test]
fn falls_back_to_concatenated_deltas_when_no_final_message() {
    let stdout = concat!(
        r#"{"type":"agent_start"}"#,
        "\n",
        r#"{"type":"assistant_text_delta","delta":"hello "}"#,
        "\n",
        r#"{"type":"assistant_text_delta","delta":"world"}"#,
        "\n",
        r#"{"type":"agent_end","ok":true}"#,
        "\n",
    );
    assert_eq!(extract_assistant_text(stdout).unwrap(), "hello world");
}

#[test]
fn ignores_non_json_and_unknown_records() {
    let stdout = concat!(
        "not json at all\n",
        r#"{"type":"tool_result","text":"ignored"}"#,
        "\n",
        "\n",
        r#"{"type":"assistant_message","text":"clean"}"#,
        "\n",
    );
    assert_eq!(extract_assistant_text(stdout).unwrap(), "clean");
}

#[test]
fn surfaces_error_record_as_err() {
    let stdout = concat!(
        r#"{"type":"agent_start"}"#,
        "\n",
        r#"{"type":"error","message":"model stream failed"}"#,
        "\n",
        r#"{"type":"agent_end","ok":false}"#,
        "\n",
    );
    let err = extract_assistant_text(stdout).unwrap_err().to_string();
    assert!(err.contains("model stream failed"), "{err}");
}

#[test]
fn agent_end_not_ok_without_error_record_is_an_error() {
    let stdout = concat!(
        r#"{"type":"agent_start"}"#,
        "\n",
        r#"{"type":"agent_end","ok":false}"#,
        "\n"
    );
    assert!(extract_assistant_text(stdout).is_err());
}

#[test]
fn empty_output_is_an_error() {
    assert!(extract_assistant_text("").is_err());
    assert!(extract_assistant_text("   \n\n").is_err());
}

#[test]
fn empty_assistant_message_does_not_mask_a_provider_error() {
    // Real pi-agent shape on an HTTP 404: error, then an EMPTY assistant_message,
    // then agent_end ok:true. The empty answer must not swallow the 404.
    let stdout = concat!(
        r#"{"cwd":"/x","model":"MiniMax-M3-highspeed","type":"agent_start"}"#,
        "\n",
        r#"{"message":"http 404 Not Found: model `MiniMax-M3-highspeed` does not exist","type":"error"}"#,
        "\n",
        r#"{"text":"","type":"assistant_message"}"#,
        "\n",
        r#"{"ok":true,"type":"agent_end"}"#,
        "\n",
    );
    let err = extract_assistant_text(stdout).unwrap_err().to_string();
    assert!(err.contains("404"), "{err}");
}

#[test]
fn explicit_empty_message_without_error_is_ok_empty() {
    let stdout = concat!(
        r#"{"type":"assistant_message","text":""}"#,
        "\n",
        r#"{"type":"agent_end","ok":true}"#,
        "\n",
    );
    assert_eq!(extract_assistant_text(stdout).unwrap(), "");
}

#[test]
fn final_message_wins_over_earlier_error() {
    // A run that recovered: an error record appeared but a final answer followed.
    let stdout = concat!(
        r#"{"type":"error","message":"transient"}"#,
        "\n",
        r#"{"type":"assistant_message","text":"recovered answer"}"#,
        "\n",
    );
    assert_eq!(extract_assistant_text(stdout).unwrap(), "recovered answer");
}

// ---- end-to-end subprocess path (fake pi-agent) -------------------------

#[cfg(unix)]
mod subprocess {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// A fresh temp dir holding an executable shell script that stands in for
    /// `pi-agent`. Cleaned up on drop (mirrors `mcp_test::TmpStore`).
    struct FakeAgent {
        dir: PathBuf,
        script: PathBuf,
    }

    impl FakeAgent {
        fn new(body: &str) -> Self {
            Self::in_dir(|_| body.to_string())
        }

        /// Like [`new`], but builds the script body with the fixture dir
        /// already known, so the fake can reference files next to itself
        /// (e.g. report its PID into `<dir>/pid` before blocking).
        fn in_dir<F>(body: F) -> Self
        where
            F: FnOnce(&Path) -> String,
        {
            let dir = std::env::temp_dir().join(format!("kg-pi-agent-test-{}", nanoid::nanoid!()));
            std::fs::create_dir_all(&dir).unwrap();
            let body = body(&dir);
            let script = dir.join("pi-agent");
            std::fs::write(&script, body).unwrap();
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
            FakeAgent { dir, script }
        }

        fn backend(&self) -> PiAgentBackend {
            PiAgentBackend::with_binary(self.script.to_str().unwrap(), vec![])
        }
    }

    impl Drop for FakeAgent {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn complete_drives_fake_pi_agent_and_returns_final_text() {
        // Drains stdin (so the prompt write doesn't SIGPIPE) then emits JSONL.
        let fake = FakeAgent::new(
            "#!/bin/sh\ncat >/dev/null\n\
             printf '%s\\n' \
             '{\"type\":\"agent_start\"}' \
             '{\"type\":\"assistant_text_delta\",\"delta\":\"hi \"}' \
             '{\"type\":\"assistant_text_delta\",\"delta\":\"there\"}' \
             '{\"type\":\"assistant_message\",\"text\":\"hi there\"}' \
             '{\"type\":\"agent_end\",\"ok\":true}'\n",
        );
        let out = fake
            .backend()
            .complete(&[Message::user("hello")], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(out, "hi there");
    }

    #[tokio::test]
    async fn complete_forwards_the_flattened_prompt_on_stdin() {
        // Echo stdin back as the assistant text, collapsing the prompt's
        // newlines to spaces so it stays a single valid JSON line (the prompt
        // has no quotes/backslashes, so no further escaping is needed).
        let fake = FakeAgent::new(
            "#!/bin/sh\nIN=$(cat | tr '\\n' ' ')\nprintf '{\"type\":\"assistant_message\",\"text\":\"%s\"}\\n' \"$IN\"\n",
        );
        let out = fake
            .backend()
            .complete(
                &[
                    Message::system("be terse"),
                    Message::user("extract entities"),
                ],
                &CompletionOptions::default(),
            )
            .await
            .unwrap();
        // flatten_prompt: system block, blank line, then the user turn.
        assert!(out.contains("be terse"), "{out}");
        assert!(out.contains("extract entities"), "{out}");
    }

    #[tokio::test]
    async fn complete_surfaces_stream_error_on_nonzero_exit() {
        let fake = FakeAgent::new(
            "#!/bin/sh\ncat >/dev/null\n\
             printf '%s\\n' \
             '{\"type\":\"error\",\"message\":\"boom from model\"}' \
             '{\"type\":\"agent_end\",\"ok\":false}'\nexit 1\n",
        );
        let err = fake
            .backend()
            .complete(&[Message::user("hi")], &CompletionOptions::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("boom from model"), "{err}");
    }

    #[tokio::test]
    async fn complete_reports_stderr_on_pre_stream_failure() {
        // No stdout JSON at all — mimics a usage/credential error printed to
        // stderr before the stream starts.
        let fake = FakeAgent::new(
            "#!/bin/sh\ncat >/dev/null\necho 'pi-agent: missing API key' 1>&2\nexit 2\n",
        );
        let err = fake
            .backend()
            .complete(&[Message::user("hi")], &CompletionOptions::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing API key"), "{err}");
    }

    #[tokio::test]
    async fn complete_errors_when_binary_is_missing() {
        let backend = PiAgentBackend::with_binary("definitely-not-a-real-pi-agent-xyz", vec![]);
        let err = backend
            .complete(&[Message::user("hi")], &CompletionOptions::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("failed to spawn"), "{err}");
    }

    // ---- cancellation owns the spawned child -------------------------------

    /// A fake that reports its PID into `<dir>/pid` and then blocks for a long
    /// time without ever answering — the stand-in for a pi-agent mid-run. It
    /// `exec`s into `sleep` so the reported PID is the process being killed
    /// (no sh/sleep descendant pair to leave behind).
    fn blocking_agent() -> FakeAgent {
        FakeAgent::in_dir(|dir| {
            format!(
                "#!/bin/sh\necho $$ > {}/pid\nexec sleep 60\n",
                dir.display()
            )
        })
    }

    /// Poll `fut` (without consuming it) until the fake announced its PID,
    /// panicking if `complete` returns first — the fake never answers.
    async fn wait_until_spawned<F>(pidfile: &Path, fut: &mut F)
    where
        F: std::future::Future<Output = anyhow::Result<String>> + Unpin,
    {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !pidfile.exists() {
            assert!(Instant::now() < deadline, "fake pi-agent never spawned");
            tokio::select! {
                res = &mut *fut => panic!("complete returned while fake blocks: {res:?}"),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    }

    /// Wait until `kill -0 <pid>` fails: the child exited AND was reaped (an
    /// unreaped zombie would still answer `kill -0`).
    async fn assert_child_exited_and_reaped(pidfile: &Path) {
        let pid: String = std::fs::read_to_string(pidfile).unwrap().trim().to_string();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !alive {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "canceled pi-agent (pid {pid}) still alive or unreaped"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn cancel_while_waiting_for_output_kills_and_reaps_child() {
        let fake = blocking_agent();
        let pidfile = fake.dir.join("pid");
        let backend = fake.backend();
        // Small prompt fits the pipe buffer: stdin is delivered instantly, so
        // the future parks waiting for the child's output.
        let messages = [Message::user("hi")];
        let options = CompletionOptions::default();
        let mut fut = Box::pin(backend.complete(&messages, &options));
        wait_until_spawned(&pidfile, &mut fut).await;
        // Drop = the caller cancels the extraction.
        drop(fut);
        assert_child_exited_and_reaped(&pidfile).await;
    }

    #[tokio::test]
    async fn cancel_during_stdin_write_kills_and_reaps_child() {
        let fake = blocking_agent();
        let pidfile = fake.dir.join("pid");
        let backend = fake.backend();
        // The fake never reads stdin; a prompt far larger than the pipe buffer
        // (64 KiB on macOS) parks the future inside the stdin write_all.
        let big_prompt = "x".repeat(256 * 1024);
        let messages = [Message::user(big_prompt)];
        let options = CompletionOptions::default();
        let mut fut = Box::pin(backend.complete(&messages, &options));
        wait_until_spawned(&pidfile, &mut fut).await;
        // One more poll round so the write fills the pipe buffer and parks.
        tokio::select! {
            res = fut.as_mut() => panic!("complete returned while fake blocks: {res:?}"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        // Drop = the caller cancels the extraction mid-write.
        drop(fut);
        assert_child_exited_and_reaped(&pidfile).await;
    }
}
