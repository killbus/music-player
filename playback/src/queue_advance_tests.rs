//! Real native resolver -> HTTP MP3 -> engine -> daemon queue policy.
//! Loopback PCM acceptance is not a real Emby/device listening test.
use super::*;
use migration::{Migrator, MigratorTrait};
use music_player_storage::{saved_servers, Database};
use music_player_types::source::{RemoteIdentity, SourceRef};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

const AUDIO: &[u8] = include_bytes!("../tests/fixtures/tone-2s.mp3");
static PLAYBACK_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Copy, Debug)]
enum Response {
    Complete,
    Short,
    Truncated,
    Denied,
    Recovering,
    InvalidBody,
    MissingAudio,
    MissingPinnedAudio,
    AuthDenied,
}

struct Fixture {
    player: PlayerInternal,
    requests: Arc<Mutex<Vec<String>>>,
    stream_targets: Arc<Mutex<Vec<String>>>,
    events: UnboundedReceiver<PlayerEvent>,
    tasks: Vec<JoinHandle<()>>,
    _test_lock: tokio::sync::MutexGuard<'static, ()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn read_request(socket: &mut TcpStream) -> (String, String) {
    let mut bytes = Vec::new();
    let mut buffer = [0; 2048];
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            break end + 4;
        }
        assert!(bytes.len() < 65536);
        let n = socket.read(&mut buffer).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&buffer[..n]);
    };
    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    assert!(length < 65536);
    while bytes.len() < end + length {
        let n = socket.read(&mut buffer).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&buffer[..n]);
    }
    let mut line = headers.lines().next().unwrap().split_ascii_whitespace();
    (line.next().unwrap().into(), line.next().unwrap().into())
}

impl Fixture {
    async fn new(response: Response, repeat: i32) -> Self {
        Self::with_volume(response, repeat, 100).await
    }

    async fn with_volume(response: Response, repeat: i32, volume: u16) -> Self {
        let test_lock = PLAYBACK_TEST_LOCK.lock().await;
        let connection = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&connection, None).await.unwrap();
        let db = Database { connection };
        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", http.local_addr().unwrap());
        let account = saved_servers::upsert(
            db.get_connection(),
            &saved_servers::NewServer::new("emby", "queue fixture", &url)
                .with_credentials(Some("family".into()), None)
                .with_password_update(saved_servers::PasswordUpdate::Set(String::new())),
            "2026-11-06T00:00:00Z",
        )
        .await
        .unwrap();
        let remote = RemoteIdentity {
            server_id: "server-a".into(),
            user_id: "user-a".into(),
        };
        saved_servers::bind_remote_identity(db.get_connection(), &account, &remote)
            .await
            .unwrap();
        let tracks = ["a", "b"].map(|id| {
            let handle = SourceRef {
                resolver: "emby".into(),
                account_id: account.id.clone(),
                remote: remote.clone(),
                kind: ResourceKind::Item,
                item_id: id.into(),
            }
            .to_handle();
            Track {
                id: handle.clone(),
                uri: handle,
                title: id.into(),
                ..Default::default()
            }
        });
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let stream_targets = Arc::new(Mutex::new(Vec::new()));
        let recorded_targets = stream_targets.clone();
        let server = tokio::spawn(async move {
            let mut resolutions = 0;
            loop {
                let (mut socket, _) = http.accept().await.unwrap();
                let (method, target) = read_request(&mut socket).await;
                let path = target.split('?').next().unwrap();
                let duration = if matches!(response, Response::Short | Response::Truncated | Response::MissingPinnedAudio) {
                    600_000_000
                } else {
                    20_000_000
                };
                let mut status = "200 OK";
                let mut extra_length = 0;
                let body = if method == "POST" && path == "/Users/AuthenticateByName" {
                    if matches!(response, Response::AuthDenied) { status = "401 Unauthorized"; }
                    serde_json::to_vec(&json!({"AccessToken":"fixture-token", "ServerId":"server-a", "User":{"Id":"user-a"}})).unwrap()
                } else if let Some(id) = path.strip_prefix("/Users/user-a/Items/") {
                    serde_json::to_vec(&json!({"Id":id, "Name":id, "Type":"Episode", "MediaType":"Video", "RunTimeTicks":duration, "IsFolder":false})).unwrap()
                } else if path.starts_with("/Items/") && path.ends_with("/PlaybackInfo") {
                    resolutions += 1;
                    serde_json::to_vec(
                        &json!({"PlaySessionId":format!("session-{resolutions}"), "MediaSources":[{
                            "Id":"version-a", "RunTimeTicks":duration, "DefaultAudioStreamIndex":1,
                            "MediaStreams": if (matches!(response, Response::MissingAudio) && resolutions == 1) || (matches!(response, Response::MissingPinnedAudio) && resolutions == 2) { json!([]) } else { json!([{"Type":"Audio", "Index":1, "Codec":"aac"}]) }
                        }]}),
                    )
                    .unwrap()
                } else if path.starts_with("/Audio/") && path.ends_with("/stream.mp3") {
                    let id = path
                        .trim_start_matches("/Audio/")
                        .trim_end_matches("/stream.mp3");
                    recorded.lock().unwrap().push(id.into());
                    recorded_targets.lock().unwrap().push(target.clone());
                    assert!(target.contains("AudioStreamIndex=1"));
                    if !matches!(response, Response::Short | Response::Truncated | Response::MissingPinnedAudio) {
                        assert!(target.contains("StartTimeTicks=0"));
                    }
                    match response {
                        Response::Denied => {
                            status = "403 Forbidden";
                            b"denied".to_vec()
                        }
                        Response::Recovering if resolutions == 1 => {
                            status = "403 Forbidden";
                            b"expired fixture stream".to_vec()
                        }
                        Response::InvalidBody if resolutions == 1 => b"<html>upstream temporarily unavailable</html>".to_vec(),
                        Response::Truncated => {
                            extra_length = 1000;
                            AUDIO.to_vec()
                        }
                        _ => AUDIO.to_vec(),
                    }
                } else if method == "DELETE" && path == "/Videos/ActiveEncodings" {
                    b"{}".to_vec()
                } else {
                    panic!("unexpected fixture request: {method} {path}");
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() + extra_length
                );
                // Cancellation may close a request before the response is written.
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.shutdown().await;
            }
        });
        let (mut player, listener) = queue_snapshot_tests::host(Tracklist::new(tracks.to_vec()));
        listener.set_nonblocking(true).unwrap();
        let listener = TcpListener::from_std(listener).unwrap();
        let collector = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 8192];
            while socket.read(&mut buffer).await.unwrap_or(0) != 0 {}
        });
        player.source_playback = Some(SourcePlayback::new(SourceResolver::new(
            db,
            "queue-ci".into(),
            true,
        )));
        player.repeat_mode = repeat;
        let (sender, events) = mpsc::unbounded_channel();
        player.event_senders.push(sender);
        player.handle_set_volume(volume);
        player.handle_next();
        Self {
            player,
            requests,
            stream_targets,
            events,
            tasks: vec![server, collector],
            _test_lock: test_lock,
        }
    }

    async fn until(&mut self, condition: impl Fn(&Self) -> bool) {
        // This spins a real resolver, decoder and TCP output. Workspace coverage
        // runs those under heavy parallel instrumentation on shared CI workers.
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                for task in &self.tasks {
                    assert!(!task.is_finished(), "fixture worker exited early");
                }
                self.player.poll_engine();
                if condition(self) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "queue did not reach expected state: phase={:?}, position_ms={}, requests={:?}",
                self.player.source_phase,
                self.player.position_ms,
                *self.requests.lock().unwrap()
            )
        });
    }

    async fn settle(&mut self) {
        for _ in 0..30 {
            self.player.poll_engine();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn ends(&mut self) -> Vec<bool> {
        let mut ends = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            if let PlayerEvent::EndOfTrack { is_last_track } = event {
                ends.push(is_last_track);
            }
        }
        ends
    }
}

#[tokio::test]
async fn managed_queue_automatically_advances_once_and_stops_after_last() {
    let mut f = Fixture::new(Response::Complete, 0).await;
    f.until(|f| f.player.source_phase == Some(Phase::Stopped))
        .await;
    f.settle().await;
    assert_eq!(*f.requests.lock().unwrap(), ["a", "b"]);
    assert_eq!(f.ends(), [false, true]);
    assert!(
        !f.player
            .tracklist
            .lock()
            .unwrap()
            .playback_state()
            .is_playing
    );
    assert_eq!(
        f.player
            .tracklist
            .lock()
            .unwrap()
            .current_track()
            .0
            .unwrap()
            .title,
        "b"
    );
    let checkpoint = f
        .player
        .source_playback
        .as_ref()
        .unwrap()
        .checkpoint()
        .unwrap();
    assert_eq!(checkpoint.offset_ms, 0);
    f.player.handle_play();
    f.until(|f| f.requests.lock().unwrap().len() == 3).await;
    let replay = f
        .player
        .source_playback
        .as_ref()
        .unwrap()
        .checkpoint()
        .unwrap();
    assert_eq!(replay.occurrence_id, checkpoint.occurrence_id);
    assert_eq!(replay.selection, checkpoint.selection);
    // The HTTP fixture asserts StartTimeTicks=0 for this replay as well.
    f.until(|f| f.player.source_phase == Some(Phase::Stopped))
        .await;
    f.settle().await;
    assert_eq!(*f.requests.lock().unwrap(), ["a", "b", "b"]);
    assert_eq!(f.ends(), [true]);
}

#[tokio::test]
async fn managed_muted_tail_keeps_play_idempotent_and_unmute_advances_once() {
    let mut f = Fixture::with_volume(Response::Complete, 0, 0).await;
    f.until(|f| f.player.source_phase == Some(Phase::Draining))
        .await;
    f.settle().await;
    assert_eq!(f.player.position_ms, 0, "upstream mute freeze is retained");
    assert!(
        f.player
            .tracklist
            .lock()
            .unwrap()
            .playback_state()
            .is_playing
    );
    let generation = f
        .player
        .source_playback
        .as_ref()
        .unwrap()
        .snapshot()
        .generation;
    f.player.handle_play();
    f.settle().await;
    assert_eq!(*f.requests.lock().unwrap(), ["a"]);
    assert_eq!(
        f.player
            .source_playback
            .as_ref()
            .unwrap()
            .snapshot()
            .generation,
        generation
    );
    assert!(f.ends().is_empty());
    f.player.handle_set_volume(100);
    f.until(|f| f.player.source_phase == Some(Phase::Stopped))
        .await;
    f.settle().await;
    assert_eq!(*f.requests.lock().unwrap(), ["a", "b"]);
    assert_eq!(f.ends(), [false, true]);
}

#[tokio::test]
async fn managed_manual_stop_during_drain_retains_delivered_checkpoint() {
    let mut f = Fixture::new(Response::Complete, 0).await;
    f.until(|f| f.player.source_phase == Some(Phase::Draining) && f.player.position_ms > 0)
        .await;
    let before = f.player.position_ms;
    f.player.handle_player_stop();
    f.settle().await;
    let checkpoint = f
        .player
        .source_playback
        .as_ref()
        .unwrap()
        .checkpoint()
        .unwrap();
    assert!(checkpoint.offset_ms >= u64::from(before));
    assert!(checkpoint.offset_ms < 2000);
    assert_eq!(f.player.source_phase, Some(Phase::Stopped));
    assert_eq!(*f.requests.lock().unwrap(), ["a"]);
    assert!(f.ends().is_empty());
}

#[tokio::test]
async fn acknowledged_modes_preserve_managed_current_and_shuffle_off_keeps_order() {
    let mut f = Fixture::with_volume(Response::Complete, 0, 0).await;
    f.until(|f| f.player.source_phase == Some(Phase::Draining))
        .await;
    let current = f.player.tracklist.lock().unwrap().current_entry();
    let generation = f
        .player
        .source_playback
        .as_ref()
        .unwrap()
        .snapshot()
        .generation;
    let (reply, received) = tokio::sync::oneshot::channel();
    f.player
        .handle_command(PlayerCommand::SetPlaybackModes {
            shuffle: Some(true),
            repeat_mode: Some(1),
            reply,
        })
        .unwrap();
    assert_eq!(received.await.unwrap(), Ok(()));
    let shuffled = f.player.tracklist.lock().unwrap().entries();
    for enabled in [true, false] {
        let (reply, received) = tokio::sync::oneshot::channel();
        f.player
            .handle_command(PlayerCommand::SetPlaybackModes {
                shuffle: Some(enabled),
                repeat_mode: None,
                reply,
            })
            .unwrap();
        assert_eq!(received.await.unwrap(), Ok(()));
        let queue = f.player.tracklist.lock().unwrap();
        assert_eq!(queue.current_entry(), current);
        assert_eq!(queue.entries(), shuffled);
        assert_eq!(queue.shuffle_enabled(), enabled);
        assert_eq!(queue.repeat_mode(), 1);
    }
    assert_eq!(
        f.player
            .source_playback
            .as_ref()
            .unwrap()
            .snapshot()
            .generation,
        generation
    );
    assert_eq!(*f.requests.lock().unwrap(), ["a"]);
    // A queued request abandoned before dispatch must not change the mode.
    let (reply, received) = tokio::sync::oneshot::channel();
    drop(received);
    f.player
        .handle_command(PlayerCommand::SetPlaybackModes {
            shuffle: Some(true),
            repeat_mode: Some(2),
            reply,
        })
        .unwrap();
    assert!(!f.player.shuffle);
    assert_eq!(f.player.repeat_mode, 1);
    f.player.handle_set_volume(100);
    f.until(|f| f.requests.lock().unwrap().len() == 3).await;
    f.player.handle_pause();
    assert_eq!(*f.requests.lock().unwrap(), ["a", "b", "a"]);
}

#[tokio::test]
async fn managed_queue_repeat_one_reopens_current_and_repeat_all_wraps() {
    for (repeat, expected) in [(2, vec!["a", "a"]), (1, vec!["a", "b", "a"])] {
        let mut f = Fixture::new(Response::Complete, repeat).await;
        f.until(|f| f.requests.lock().unwrap().len() == expected.len())
            .await;
        f.player.handle_pause();
        f.settle().await;
        assert_eq!(*f.requests.lock().unwrap(), expected);
        assert_eq!(f.ends(), vec![false; expected.len() - 1]);
        assert_eq!(f.player.source_phase, Some(Phase::Paused));
    }
}

#[tokio::test]
async fn managed_queue_does_not_skip_short_clean_eof_transport_failure_or_http_error() {
    for response in [Response::Short, Response::Truncated, Response::Denied] {
        let mut f = Fixture::new(response, 0).await;
        f.until(|f| {
            f.player.source_phase == Some(Phase::Failed)
        })
        .await;
        f.settle().await;
        assert_eq!(*f.requests.lock().unwrap(), ["a", "a", "a"], "{response:?}: {:?}", f.player.source_playback.as_ref().unwrap().status());
        assert_eq!(f.player.source_playback.as_ref().unwrap().status().unwrap().retry_attempt, 2);
        assert!(f.ends().is_empty());
        assert_eq!(
            f.player
                .tracklist
                .lock()
                .unwrap()
                .current_track()
                .0
                .unwrap()
                .title,
            "a"
        );
        assert_eq!(f.player.tracklist.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn managed_queue_manual_next_and_pause_cannot_trigger_stale_advance() {
    let mut f = Fixture::new(Response::Complete, 0).await;
    // The short fixture may reach decoder EOF before the first host tick;
    // delivered PCM, not the transient phase, proves A actually started.
    f.until(|f| f.player.position_ms > 0).await;
    f.player.handle_next();
    f.until(|f| f.requests.lock().unwrap().len() == 2).await;
    f.player.handle_pause();
    f.settle().await;
    assert_eq!(*f.requests.lock().unwrap(), ["a", "b"]);
    assert!(f.ends().is_empty());
    assert_eq!(f.player.source_phase, Some(Phase::Paused));
    assert_eq!(
        f.player
            .tracklist
            .lock()
            .unwrap()
            .current_track()
            .0
            .unwrap()
            .title,
        "b"
    );
}

#[tokio::test]
async fn failed_stream_automatically_resolves_a_fresh_session() {
    let mut f = Fixture::new(Response::Recovering, 0).await;
    f.until(|f| f.player.source_phase == Some(Phase::Recovering))
        .await;
    let current = f.player.tracklist.lock().unwrap().current_entry().unwrap();
    let (reply, received) = tokio::sync::oneshot::channel();
    f.player
        .handle_command(PlayerCommand::GetSourcePlayback { reply })
        .unwrap();
    let status = received.await.unwrap().unwrap();
    assert_eq!(status.phase, Phase::Recovering);
    assert_eq!(status.occurrence_id, current.occurrence_id);
    assert!(status.error.is_none());
    assert_eq!(status.retry_attempt, 1);
    assert!(
        !f.player
            .tracklist
            .lock()
            .unwrap()
            .playback_state()
            .is_playing
    );

    // No manual Play: the server owns recovery.
    f.until(|f| f.player.position_ms > 0).await;
    f.player.handle_pause();
    assert_eq!(*f.requests.lock().unwrap(), ["a", "a"]);
    let targets = f.stream_targets.lock().unwrap();
    assert!(targets[0].contains("PlaySessionId=session-1"));
    assert!(targets[1].contains("PlaySessionId=session-2"));
    drop(targets);
    assert_eq!(
        f.player
            .tracklist
            .lock()
            .unwrap()
            .current_entry()
            .unwrap()
            .occurrence_id,
        current.occurrence_id
    );
    assert!(f.ends().is_empty());
}

#[tokio::test]
async fn pause_resume_reopens_with_the_frozen_output_checkpoint_and_pin() {
    let mut f = Fixture::new(Response::Short, 0).await;
    f.until(|f| f.player.position_ms > 0).await;
    f.player.handle_pause();
    let paused = f
        .player
        .source_playback
        .as_ref()
        .unwrap()
        .checkpoint()
        .unwrap();
    assert!(paused.offset_ms > 0);
    f.settle().await;
    assert_eq!(f.player.position_ms as u64, paused.offset_ms);
    assert_eq!(f.requests.lock().unwrap().len(), 1);

    f.player.handle_play();
    f.until(|f| f.player.position_ms as u64 > paused.offset_ms)
        .await;
    let resumed = f
        .player
        .source_playback
        .as_ref()
        .unwrap()
        .checkpoint()
        .unwrap();
    f.player.handle_pause();
    assert_eq!(resumed.occurrence_id, paused.occurrence_id);
    assert_eq!(resumed.selection, paused.selection);
    assert_eq!(*f.requests.lock().unwrap(), ["a", "a"]);
    let targets = f.stream_targets.lock().unwrap();
    assert!(targets[1].contains(&format!("StartTimeTicks={}", paused.offset_ms * 10_000)));
    assert!(targets[1].contains("PlaySessionId=session-2"));
    drop(targets);
    assert!(f.ends().is_empty());
}

#[tokio::test]
async fn ambiguous_decoder_and_metadata_failures_recover_without_manual_play() {
    for response in [Response::InvalidBody, Response::MissingAudio] {
        let mut f = Fixture::new(response, 0).await;
        f.until(|f| f.player.source_phase == Some(Phase::Recovering)).await;
        let occurrence = f.player.tracklist.lock().unwrap().current_entry().unwrap().occurrence_id;
        f.until(|f| f.player.position_ms > 0).await;
        f.player.handle_pause();
        assert_eq!(f.player.tracklist.lock().unwrap().current_entry().unwrap().occurrence_id, occurrence);
        assert!(f.stream_targets.lock().unwrap().last().unwrap().contains("PlaySessionId=session-2"));
        assert!(f.ends().is_empty());
    }
}

#[tokio::test]
async fn pause_stop_and_next_cancel_recovery_backoff() {
    for command in 0..3 {
        let mut f = Fixture::new(Response::Recovering, 0).await;
        f.until(|f| f.player.source_phase == Some(Phase::Recovering)).await;
        // A duplicate Play must not bypass the pending delay.
        f.player.handle_play();
        match command {
            0 => f.player.handle_pause(),
            1 => f.player.handle_player_stop(),
            _ => f.player.handle_next(),
        }
        for _ in 0..7 { f.settle().await; }
        let requests = f.requests.lock().unwrap().clone();
        assert_eq!(requests.iter().filter(|id| *id == "a").count(), 1);
        if command == 2 {
            assert_eq!(requests, ["a", "b"]);
        } else {
            assert_eq!(requests, ["a"]);
            assert!(f.ends().is_empty());
        }
    }
}

#[tokio::test]
async fn explicit_api_auth_denial_is_not_automatically_retried() {
    let mut f = Fixture::new(Response::AuthDenied, 0).await;
    f.until(|f| f.player.source_phase == Some(Phase::Failed)).await;
    for _ in 0..5 { f.settle().await; }
    let status = f.player.source_playback.as_ref().unwrap().status().unwrap();
    assert_eq!(status.retry_attempt, 0);
    assert!(f.requests.lock().unwrap().is_empty());
    assert!(f.ends().is_empty());
}

#[tokio::test]
async fn automatic_resume_preserves_delivered_offset_and_pin_across_missing_metadata() {
    let mut f = Fixture::new(Response::MissingPinnedAudio, 0).await;
    f.until(|f| f.player.source_phase == Some(Phase::Recovering)).await;
    let checkpoint = f.player.source_playback.as_ref().unwrap().checkpoint().unwrap();
    assert!(checkpoint.offset_ms > 0);
    assert!(matches!(checkpoint.selection, music_player_types::audio::AudioSelection::Pinned(_)));
    f.until(|f| f.stream_targets.lock().unwrap().len() == 2 && u64::from(f.player.position_ms) > checkpoint.offset_ms).await;
    f.player.handle_pause();
    let resumed = f.player.source_playback.as_ref().unwrap().checkpoint().unwrap();
    assert_eq!(resumed.selection, checkpoint.selection);
    assert_eq!(resumed.occurrence_id, checkpoint.occurrence_id);
    let targets = f.stream_targets.lock().unwrap();
    assert!(targets[1].contains("PlaySessionId=session-3"));
    assert!(targets[1].contains(&format!("StartTimeTicks={}", checkpoint.offset_ms * 10_000)));
    drop(targets);
    assert!(f.ends().is_empty());
}
