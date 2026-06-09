use super::*;

// ---- name routing -------------------------------------------------------

#[test]
fn accepts_recognizes_pi_agent_aliases() {
    for name in ["pi-agent", "pi_agent", "piagent", "pi", "PI-AGENT", "  Pi-Agent  "] {
        assert!(PiAgentBackend::accepts(name), "{name:?} should select pi-agent");
    }
    for name in ["minimaxcc", "glmcc", "mimocc", "pip", "", "agent"] {
        assert!(!PiAgentBackend::accepts(name), "{name:?} should not select pi-agent");
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
    let stdout = concat!(r#"{"type":"agent_start"}"#, "\n", r#"{"type":"agent_end","ok":false}"#, "\n");
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
    use std::path::PathBuf;

    /// A fresh temp dir holding an executable shell script that stands in for
    /// `pi-agent`. Cleaned up on drop (mirrors `mcp_test::TmpStore`).
    struct FakeAgent {
        dir: PathBuf,
        script: PathBuf,
    }

    impl FakeAgent {
        fn new(body: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("kg-pi-agent-test-{}", nanoid::nanoid!()));
            std::fs::create_dir_all(&dir).unwrap();
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
                &[Message::system("be terse"), Message::user("extract entities")],
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
}
