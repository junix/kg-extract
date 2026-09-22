use super::*;

#[tokio::test]
async fn backends_have_no_native_session_by_default() {
    let backend = MockBackend::single("x");
    let session = backend
        .open_session(None, &CompletionOptions::default())
        .await
        .unwrap();
    assert!(
        session.is_none(),
        "default open_session must be None so callers replay"
    );
}

#[tokio::test]
async fn replay_session_returns_canned_replies_in_order_and_grows_history() {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::new(vec!["first".into(), "second".into()]));
    let mut session = ReplaySession::new(
        backend.clone(),
        Some("sys".into()),
        CompletionOptions::default(),
    );

    assert_eq!(session.send("turn one").await.unwrap(), "first");
    assert_eq!(session.send("turn two").await.unwrap(), "second");
    // After two turns the replayed history is system + (user/assistant) x2.
    assert_eq!(session.history.len(), 5);
    assert_eq!(session.history[0].role, "system");
    assert_eq!(session.history[1].content, "turn one");
    assert_eq!(session.history[2].content, "first");
    session.finish().await.unwrap(); // default no-op
}
