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

#[derive(Clone, Copy)]
enum Response {
    Complete,
    Short,
    Truncated,
    Denied,
}

struct Fixture {
    player: PlayerInternal,
    requests: Arc<Mutex<Vec<String>>>,
    events: UnboundedReceiver<PlayerEvent>,
    tasks: Vec<JoinHandle<()>>,
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
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = http.accept().await.unwrap();
                let (method, target) = read_request(&mut socket).await;
                let path = target.split('?').next().unwrap();
                let duration = if matches!(response, Response::Short) {
                    600_000_000
                } else {
                    20_000_000
                };
                let mut status = "200 OK";
                let mut extra_length = 0;
                let body = if method == "POST" && path == "/Users/AuthenticateByName" {
                    serde_json::to_vec(&json!({"AccessToken":"fixture-token", "ServerId":"server-a", "User":{"Id":"user-a"}})).unwrap()
                } else if let Some(id) = path.strip_prefix("/Users/user-a/Items/") {
                    serde_json::to_vec(&json!({"Id":id, "Name":id, "Type":"Episode", "MediaType":"Video", "RunTimeTicks":duration, "IsFolder":false})).unwrap()
                } else if path.starts_with("/Items/") && path.ends_with("/PlaybackInfo") {
                    serde_json::to_vec(&json!({"PlaySessionId":"session-fixture", "MediaSources":[{
                        "Id":"version-a", "RunTimeTicks":duration, "DefaultAudioStreamIndex":1,
                        "MediaStreams":[{"Type":"Audio", "Index":1, "Codec":"aac"}]
                    }]}))
                    .unwrap()
                } else if path.starts_with("/Audio/") && path.ends_with("/stream.mp3") {
                    let id = path
                        .trim_start_matches("/Audio/")
                        .trim_end_matches("/stream.mp3");
                    recorded.lock().unwrap().push(id.into());
                    assert!(target.contains("AudioStreamIndex=1"));
                    assert!(target.contains("StartTimeTicks=0"));
                    match response {
                        Response::Denied => {
                            status = "403 Forbidden";
                            b"denied".to_vec()
                        }
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
        player.handle_next();
        Self {
            player,
            requests,
            events,
            tasks: vec![server, collector],
        }
    }

    async fn until(&mut self, condition: impl Fn(&Self) -> bool) {
        tokio::time::timeout(Duration::from_secs(20), async {
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
        .expect("queue did not reach expected state");
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
                || (f.player.source_phase == Some(Phase::EndUnconfirmed)
                    && f.player.position_ms >= 1900)
        })
        .await;
        f.settle().await;
        assert_eq!(*f.requests.lock().unwrap(), ["a"]);
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
    f.until(|f| f.player.source_phase == Some(Phase::Streaming))
        .await;
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
