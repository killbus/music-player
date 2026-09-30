use super::*;
use music_player_provider::emby_playback::AudioPin;
use music_player_storage::Database;
use music_player_types::source::RemoteIdentity;

async fn playback() -> SourcePlayback {
    let connection = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
    SourcePlayback::new(SourceResolver::new(
        Database { connection },
        "fixture".into(),
        true,
    ))
}

fn source(id: &str) -> SourceRef {
    SourceRef {
        resolver: "emby".into(),
        account_id: "saved-account".into(),
        remote: RemoteIdentity {
            server_id: "server-a".into(),
            user_id: "family-id".into(),
        },
        kind: ResourceKind::Item,
        item_id: id.into(),
    }
}

fn roundtrip(value: SourceCheckpoint) -> SourceCheckpoint {
    let raw = serde_json::to_string(&value).unwrap();
    // No authenticated request/transport state belongs in this document.
    assert!(!raw.contains("http:"));
    assert!(!raw.contains("headers"));
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn pending_seek_survives_restart_before_first_resolution_without_network() {
    let mut first = playback().await;
    first.load(source("55508"), Duration::ZERO, false).unwrap();
    first.seek(Duration::from_millis(7_200_123));
    let checkpoint = roundtrip(first.checkpoint().unwrap());
    assert!(matches!(checkpoint.selection, AudioSelection::Auto));
    let mut restored = playback().await;
    restored.restore(source("55508"), checkpoint).unwrap();
    assert_eq!(restored.snapshot().desired, DesiredState::Paused);
    assert_eq!(restored.snapshot().target, Duration::from_millis(7_200_123));
    assert!(restored.task.is_none());
    assert!(restored.snapshot().position.is_none());
    assert!(restored.completed.try_recv().is_err());
}

#[tokio::test]
async fn pinned_selection_and_u64_position_survive_restore_seek_and_stop() {
    let pin = AudioPin {
        media_source_id: "version-a".into(),
        audio_stream_index: 1,
        runtime_ticks: Some(100_000_000_000_000),
        etag: Some("revision-a".into()),
        codec: Some("aac".into()),
        channels: Some(2),
        sample_rate: Some(44100),
    };
    let position = u64::from(u32::MAX) + 7_200_123;
    let checkpoint = SourceCheckpoint {
        version: 1,
        occurrence_id: uuid::Uuid::new_v4().to_string(),
        source: source("55508").to_handle(),
        offset_ms: position,
        selection: AudioSelection::Pinned(pin.clone()),
    };
    let mut player = playback().await;
    player
        .restore(source("55508"), roundtrip(checkpoint))
        .unwrap();
    assert_eq!(player.snapshot().target.as_millis(), u128::from(position));
    player.seek(Duration::from_millis(position + 321));
    player.stop();
    let saved = roundtrip(player.checkpoint().unwrap());
    assert_eq!(saved.offset_ms, position + 321);
    assert_eq!(saved.selection, AudioSelection::Pinned(pin));
    assert_eq!(player.snapshot().desired, DesiredState::Stopped);
    assert!(player.task.is_none());
}

#[tokio::test]
async fn incompatible_checkpoint_cannot_replace_the_current_occurrence() {
    let mut player = playback().await;
    player
        .load(source("current"), Duration::ZERO, false)
        .unwrap();
    let original = player.checkpoint().unwrap();
    for case in 0..3 {
        let mut invalid = original.clone();
        match case {
            0 => invalid.source = source("other").to_handle(),
            1 => invalid.version = 99,
            _ => invalid.occurrence_id = "invalid-uuid".into(),
        }
        assert!(player.restore(source("current"), invalid).is_err());
        assert_eq!(
            player.checkpoint().unwrap().occurrence_id,
            original.occurrence_id
        );
    }
}

#[tokio::test]
async fn repeated_item_is_a_new_occurrence_and_old_error_does_not_replace_it() {
    let mut player = playback().await;
    player.load(source("55508"), Duration::ZERO, false).unwrap();
    // Supply controlled late completions on the actual bounded mailbox. No
    // engine may start for an error, and no network timing is needed here.
    let old_id = player.occurrence.as_ref().unwrap().id.clone();
    let old = player.coordinator.play().unwrap();
    player.load(source("55508"), Duration::ZERO, false).unwrap();
    assert_ne!(old_id, player.occurrence.as_ref().unwrap().id);
    let current = player.coordinator.play().unwrap();
    assert!(old.is_cancelled());
    assert!(player
        .sender
        .try_send((old, Err(ProviderError::Other("old failure".into()))))
        .is_ok());
    player.poll_with(|_| panic!("error completion attempted to open a stream"));
    assert_eq!(player.snapshot().phase, Phase::Resolving);
    assert!(player.error().is_none());
    assert!(player
        .sender
        .try_send((current, Err(ProviderError::Other("current failure".into()))))
        .is_ok());
    player.poll_with(|_| panic!("error completion attempted to open a stream"));
    assert_eq!(player.snapshot().phase, Phase::Failed);
    assert_eq!(player.error(), Some("current failure"));
}
