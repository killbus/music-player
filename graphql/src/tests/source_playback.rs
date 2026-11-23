use crate::schema::playback::PlaybackQuery;
use async_graphql::{EmptyMutation, EmptySubscription, Schema};
use music_player_playback::{
    managed::Phase,
    player::{
        PlayerCommand, PlayerRuntime, PlayerRuntimePhase, PlayerRuntimeSnapshot,
        SourcePlaybackStatus,
    },
};
use music_player_renderer::CurrentReceiverDevice;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

const QUERY: &str = "{ sourcePlayback { source occurrenceId phase error retryAttempt retryLimit } }";

fn fixture() -> (
    Schema<PlaybackQuery, EmptyMutation, EmptySubscription>,
    mpsc::UnboundedReceiver<PlayerCommand>,
    PlayerRuntime,
) {
    let (sender, commands) = mpsc::unbounded_channel();
    let runtime = Arc::new(Mutex::new(PlayerRuntimeSnapshot {
        phase: PlayerRuntimePhase::Ready,
        output: "fixture".into(),
        error: None,
    }));
    let schema = Schema::build(PlaybackQuery, EmptyMutation, EmptySubscription)
        .data(Arc::new(Mutex::new(sender)))
        .data(Arc::new(AsyncMutex::new(CurrentReceiverDevice::new())))
        .data(runtime.clone())
        .finish();
    (schema, commands, runtime)
}

#[tokio::test]
async fn ready_output_does_not_hide_a_failed_source_attempt() {
    let (schema, mut commands, _) = fixture();
    let (response, ()) = tokio::join!(schema.execute(QUERY), async {
        let PlayerCommand::GetSourcePlayback { reply } = commands.recv().await.unwrap() else {
            panic!("expected status query")
        };
        reply
            .send(Some(SourcePlaybackStatus {
                source: "fixture-source".into(),
                occurrence_id: "fixture-occurrence".into(),
                phase: Phase::Failed,
                error: Some("Audio server returned HTTP 403".into()),
                retry_attempt: 2,
                retry_limit: 2,
            }))
            .unwrap();
    });
    assert!(response.errors.is_empty());
    let data = response.data.into_json().unwrap();
    assert_eq!(data["sourcePlayback"]["phase"], "FAILED");
    assert_eq!(data["sourcePlayback"]["retryAttempt"], 2);
    assert_eq!(data["sourcePlayback"]["retryLimit"], 2);
    assert_eq!(data["sourcePlayback"]["occurrenceId"], "fixture-occurrence");
    assert_eq!(
        data["sourcePlayback"]["error"],
        "Audio server returned HTTP 403"
    );
}

#[tokio::test]
async fn no_managed_source_returns_null_and_closed_player_is_an_error() {
    let (schema, mut commands, _) = fixture();
    let (response, ()) = tokio::join!(schema.execute(QUERY), async {
        let PlayerCommand::GetSourcePlayback { reply } = commands.recv().await.unwrap() else {
            panic!("expected status query")
        };
        reply.send(None).unwrap();
    });
    assert!(response.errors.is_empty());
    assert!(response.data.into_json().unwrap()["sourcePlayback"].is_null());
    drop(commands);
    let response = schema.execute(QUERY).await;
    assert_eq!(response.errors[0].message, "Player command channel closed");
}

#[tokio::test]
async fn failed_output_returns_its_reason_without_enqueuing_a_read() {
    let (schema, mut commands, runtime) = fixture();
    runtime.lock().unwrap().phase = PlayerRuntimePhase::Failed;
    runtime.lock().unwrap().error = Some("fixture output unavailable".into());
    let response = schema.execute(QUERY).await;
    assert!(response.errors[0]
        .message
        .contains("fixture output unavailable"));
    assert!(commands.try_recv().is_err());
}
