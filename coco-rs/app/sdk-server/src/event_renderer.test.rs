use super::*;

fn attempt_started(attempt: i32) -> AgentStreamEvent {
    AgentStreamEvent::ResponseAttemptStarted {
        turn_id: TurnId::from("turn-1"),
        attempt,
    }
}

fn text_delta(text: &str) -> AgentStreamEvent {
    AgentStreamEvent::TextDelta {
        turn_id: TurnId::from("turn-1"),
        delta: text.into(),
    }
}

#[test]
fn discarded_response_attempt_never_reaches_sdk_notifications() {
    let session_id = SessionId::try_new("response-attempt-session").expect("session id");
    let mut renderer = SdkEventRenderer::default();
    renderer.accumulators.insert(
        session_id.clone(),
        StreamAccumulator::new(TurnId::from("turn-1")),
    );

    assert!(
        renderer
            .render_stream_event(&session_id, attempt_started(1))
            .is_empty()
    );
    assert!(
        renderer
            .render_stream_event(&session_id, text_delta("malformed"))
            .is_empty()
    );
    assert!(
        renderer
            .render_stream_event(
                &session_id,
                AgentStreamEvent::ResponseAttemptDiscarded {
                    turn_id: TurnId::from("turn-1"),
                    attempt: 1,
                },
            )
            .is_empty()
    );

    renderer.render_stream_event(&session_id, attempt_started(2));
    renderer.render_stream_event(&session_id, text_delta("valid"));
    let committed = renderer.render_stream_event(
        &session_id,
        AgentStreamEvent::ResponseAttemptCommitted {
            turn_id: TurnId::from("turn-1"),
            attempt: 2,
        },
    );
    assert_eq!(committed.len(), 2);
    assert!(matches!(
        committed.as_slice(),
        [
            ServerNotification::ItemStarted { .. },
            ServerNotification::AgentMessageDelta(_)
        ]
    ));
}
