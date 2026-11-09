use async_graphql::{EmptySubscription, Schema};
use music_player_playback::player::{
    PlayerCommand, PlayerRuntime, PlayerRuntimePhase, PlayerRuntimeSnapshot,
};
use music_player_renderer::{local::Local, CurrentReceiverDevice};
use music_player_tracklist::Tracklist;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::schema::tracklist::{TracklistMutation, TracklistQuery};

type ModeSchema = Schema<TracklistQuery, TracklistMutation, EmptySubscription>;
struct Fixture {
    schema: ModeSchema,
    commands: mpsc::UnboundedReceiver<PlayerCommand>,
    queue: Arc<Mutex<Tracklist>>,
    runtime: PlayerRuntime,
    receiver: Arc<AsyncMutex<CurrentReceiverDevice>>,
}

impl Fixture {
    fn new() -> Self {
        let (tx, commands) = mpsc::unbounded_channel();
        let queue = Arc::new(Mutex::new(Tracklist::new_empty()));
        let runtime = Arc::new(Mutex::new(PlayerRuntimeSnapshot {
            phase: PlayerRuntimePhase::Ready,
            output: "fixture".into(),
            error: None,
        }));
        let receiver = Arc::new(AsyncMutex::new(CurrentReceiverDevice::new()));
        let schema = Schema::build(TracklistQuery, TracklistMutation, EmptySubscription)
            .data(Arc::new(Mutex::new(tx)))
            .data(queue.clone())
            .data(runtime.clone())
            .data(receiver.clone())
            .finish();
        Self {
            schema,
            commands,
            queue,
            runtime,
            receiver,
        }
    }
}

const QUERY: &str = "{ playbackModes { available unavailableReason shuffle repeat } }";

#[tokio::test]
async fn reads_authoritative_modes_without_a_current_track_and_legacy_queries_are_safe() {
    let f = Fixture::new();
    for (value, name) in [(0, "OFF"), (1, "ALL"), (2, "ONE")] {
        f.queue.lock().unwrap().set_modes(true, value);
        let response = f.schema.execute(QUERY).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json().unwrap()["playbackModes"],
            json!({
                "available": true, "unavailableReason": null, "shuffle": true, "repeat": name,
            })
        );
        let response = f.schema.execute("{ getRandom getRepeat }").await;
        assert!(response.errors.is_empty());
        assert_eq!(
            response.data.into_json().unwrap(),
            json!({ "getRandom": true, "getRepeat": value != 0 })
        );
    }
}

#[tokio::test]
async fn setters_wait_for_acknowledgement_and_send_only_the_requested_field() {
    let mut f = Fixture::new();
    for (document, expected) in [
        ("mutation { setShuffle(enabled: true) }", (Some(true), None)),
        (
            "mutation { setShuffle(enabled: false) }",
            (Some(false), None),
        ),
        ("mutation { setRepeat(mode: OFF) }", (None, Some(0))),
        ("mutation { setRepeat(mode: ALL) }", (None, Some(1))),
        ("mutation { setRepeat(mode: ONE) }", (None, Some(2))),
        ("mutation { shuffle }", (Some(true), None)),
    ] {
        let schema = f.schema.clone();
        let task = tokio::spawn(async move { schema.execute(document).await });
        let command = f.commands.recv().await.unwrap();
        assert!(!task.is_finished(), "Enqueueing is not acknowledgement");
        match command {
            PlayerCommand::SetPlaybackModes {
                shuffle,
                repeat_mode,
                reply,
            } => {
                assert_eq!((shuffle, repeat_mode), expected);
                reply.send(Ok(())).unwrap();
            }
            other => panic!("Unexpected command: {other:?}"),
        }
        let response = task.await.unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert!(f.commands.try_recv().is_err());
    }
}

#[tokio::test]
async fn unavailable_runtime_or_receiver_never_changes_the_local_queue() {
    let mut f = Fixture::new();
    for phase in [
        PlayerRuntimePhase::Starting,
        PlayerRuntimePhase::Failed,
        PlayerRuntimePhase::Ready,
    ] {
        f.runtime.lock().unwrap().phase = phase;
        if phase == PlayerRuntimePhase::Ready {
            // An unconnected renderer panics if invoked: no receiver method is called.
            f.receiver.lock().await.client = Some(Box::new(Local::new()));
        }
        let response = f.schema.execute(QUERY).await;
        assert!(response.errors.is_empty());
        let data = response.data.into_json().unwrap();
        assert_eq!(data["playbackModes"]["available"], false);
        assert_eq!(data["playbackModes"]["shuffle"], json!(null));
        assert_eq!(data["playbackModes"]["repeat"], json!(null));
        for document in [
            "mutation { setShuffle(enabled: true) }",
            "mutation { setRepeat(mode: ONE) }",
            "mutation { shuffle }",
        ] {
            assert!(!f.schema.execute(document).await.errors.is_empty());
            assert!(f.commands.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn invalid_repeat_and_closed_channels_return_errors_not_success() {
    let mut f = Fixture::new();
    assert!(!f
        .schema
        .execute("mutation { setRepeat(mode: INVALID) }")
        .await
        .errors
        .is_empty());
    assert!(f.commands.try_recv().is_err());
    let schema = f.schema.clone();
    let task =
        tokio::spawn(async move { schema.execute("mutation { setRepeat(mode: ONE) }").await });
    drop(f.commands.recv().await.unwrap());
    let response = task.await.unwrap();
    assert!(response.errors[0]
        .message
        .contains("acknowledgement channel closed"));
    f.commands.close();
    let response = f
        .schema
        .execute("mutation { setShuffle(enabled: true) }")
        .await;
    assert!(response.errors[0]
        .message
        .contains("Player command channel closed"));
}

#[tokio::test]
async fn a_stalled_player_returns_a_bounded_acknowledgement_error() {
    let mut f = Fixture::new();
    let schema = f.schema.clone();
    let task =
        tokio::spawn(async move { schema.execute("mutation { setRepeat(mode: ONE) }").await });
    let command = f.commands.recv().await.unwrap();
    let response = tokio::time::timeout(std::time::Duration::from_secs(7), task)
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors[0].message.contains("outcome uncertain"));
    match command {
        PlayerCommand::SetPlaybackModes { reply, .. } => assert!(reply.is_closed()),
        _ => panic!("Unexpected command"),
    }
}

#[tokio::test]
async fn missing_runtime_fails_closed() {
    let schema = Schema::build(TracklistQuery, TracklistMutation, EmptySubscription)
        .data(Arc::new(AsyncMutex::new(CurrentReceiverDevice::new())))
        .finish();
    let response = schema.execute(QUERY).await;
    assert!(response.errors.is_empty());
    assert_eq!(
        response.data.into_json().unwrap()["playbackModes"]["available"],
        false
    );
    assert!(!schema
        .execute("mutation { setShuffle(enabled: true) }")
        .await
        .errors
        .is_empty());
}
