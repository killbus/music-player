//! Media navigation owns its pending reads; playback remains in the daemon.
use std::{cell::RefCell, sync::Arc, time::Duration};

use music_player_server::api::{
    metadata::v1alpha1::Track,
    music::v1alpha1::{
        library_service_client::LibraryServiceClient,
        tracklist_service_client::TracklistServiceClient, AddTracksRequest, BrowseMediaRequest,
        GetMediaBrowserRequest, GetMediaContainerTracksRequest, LoadTracksRequest, MediaEntry,
    },
};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};
use tokio::sync::{mpsc::UnboundedSender, watch};
use tonic::{transport::Channel, Code, Status};

use crate::{rpc::Cmd, AppWindow, MediaCrumb, MediaItem};

const PAGE: i32 = 100;

#[derive(Clone, Copy, Debug, PartialEq)]
enum RequestPhase {
    Pending,
    Submitted,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct Ticket(Arc<watch::Sender<RequestPhase>>);

impl Ticket {
    fn new() -> Self {
        Self(Arc::new(watch::channel(RequestPhase::Pending).0))
    }
    fn current(&self) -> bool {
        *self.0.borrow() != RequestPhase::Cancelled
    }
    fn cancel(&self) {
        self.0.send_replace(RequestPhase::Cancelled);
    }
    fn transition_pending(&self, next: RequestPhase) -> bool {
        self.0.send_if_modified(|phase| {
            if *phase != RequestPhase::Pending {
                return false;
            }
            *phase = next;
            true
        })
    }
    async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        while *receiver.borrow_and_update() != RequestPhase::Cancelled {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Location {
    server: Option<String>,
    // The root has no parent handle. Titles remain unmodified display data.
    path: Vec<(Option<String>, String)>,
    offset: i32,
}

impl Default for Location {
    fn default() -> Self {
        Self {
            server: None,
            path: vec![(None, "Libraries".into())],
            offset: 0,
        }
    }
}

#[derive(Default)]
struct State {
    location: Location,
    rows: Vec<MediaEntry>,
    next: Option<i32>,
    active: Option<Ticket>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[derive(Debug)]
enum Action {
    Browse(Location),
    Queue {
        server: String,
        entry: MediaEntry,
        play: bool,
    },
}

#[derive(Debug)]
pub struct Request {
    ticket: Ticket,
    action: Action,
    tx: UnboundedSender<Cmd>,
}

#[derive(Debug)]
pub struct Commit {
    ticket: Ticket,
    server: String,
    tracks: Vec<Track>,
    play: bool,
}

enum Reply {
    Page {
        location: Location,
        entries: Vec<MediaEntry>,
        next: Option<i32>,
    },
    Queue {
        server: String,
        tracks: Vec<Track>,
        play: bool,
    },
}

fn client(channel: &Channel) -> LibraryServiceClient<Channel> {
    LibraryServiceClient::new(channel.clone())
        .max_decoding_message_size(music_player_server::LIBRARY_MESSAGE_LIMIT)
}

fn message(error: Status) -> String {
    if error.code() == Code::Unimplemented {
        "This daemon version does not support media libraries. Update the daemon to browse them."
            .into()
    } else {
        error.message().to_owned()
    }
}

fn subtitle(entry: &MediaEntry) -> String {
    let mut parts = Vec::new();
    if let Some(kind) = &entry.item_type {
        parts.push(kind.clone());
    }
    if let Some(season) = entry.season_number {
        parts.push(format!("S{season}"));
    }
    if let Some(episode) = entry.episode_number {
        parts.push(format!("E{episode}"));
    }
    if !entry.is_container && entry.track.is_none() {
        parts.push("No audio track".into());
    }
    parts.join(" · ")
}

fn render(app: &AppWindow, state: &State) {
    app.set_media_items(ModelRc::new(VecModel::from(
        state
            .rows
            .iter()
            .map(|entry| MediaItem {
                id: entry.id.clone().into(),
                title: entry.title.clone().into(),
                subtitle: subtitle(entry).into(),
                is_container: entry.is_container,
                playable: !entry.is_container && entry.track.is_some(),
            })
            .collect::<Vec<_>>(),
    )));
    app.set_media_crumbs(ModelRc::new(VecModel::from(
        state
            .location
            .path
            .iter()
            .map(|(_, title)| MediaCrumb {
                title: title.clone().into(),
            })
            .collect::<Vec<_>>(),
    )));
    app.set_media_has_next(state.next.is_some());
    app.set_media_has_previous(state.location.offset > 0);
    app.set_media_offset_text(if state.rows.is_empty() {
        "".into()
    } else {
        format!(
            "{}–{}",
            i64::from(state.location.offset) + 1,
            i64::from(state.location.offset) + state.rows.len() as i64
        )
        .into()
    });
}

/// Called synchronously by every provider/daemon switch callback, before
/// its command enters the worker queue. A -> B -> A still cancels the old A.
pub fn invalidate(app: &AppWindow) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(ticket) = state.active.take() {
            ticket.cancel();
        }
        *state = State::default();
        render(app, &state);
    });
    app.set_media_loading(false);
    app.set_media_busy(false);
    app.set_media_cancellable(false);
    app.set_media_message("Refresh to browse the connected server's media libraries.".into());
}

fn start(app: &AppWindow, tx: &UnboundedSender<Cmd>, action: Action) {
    let ticket = Ticket::new();
    let loading = matches!(&action, Action::Browse(_));
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(previous) = state.active.replace(ticket.clone()) {
            previous.cancel();
        }
        if let Action::Browse(location) = &action {
            state.location = location.clone();
            state.rows.clear();
            state.next = None;
            render(app, &state);
        }
    });
    app.set_media_loading(loading);
    app.set_media_busy(!loading);
    app.set_media_cancellable(true);
    app.set_media_message("".into());
    if tx
        .send(Cmd::Media(Request {
            ticket,
            action,
            tx: tx.clone(),
        }))
        .is_err()
    {
        app.set_media_loading(false);
        app.set_media_busy(false);
        app.set_media_cancellable(false);
        app.set_media_message("The connection worker is unavailable.".into());
    }
}

pub fn bind(app: &AppWindow, tx: &UnboundedSender<Cmd>) {
    {
        let weak = app.as_weak();
        let tx = tx.clone();
        app.on_media_refresh(move || {
            start(&weak.unwrap(), &tx, Action::Browse(Location::default()))
        });
    }
    {
        let weak = app.as_weak();
        let tx = tx.clone();
        app.on_media_open(move |index| {
            let location = STATE.with(|state| {
                let state = state.borrow();
                let row = state.rows.get(usize::try_from(index).ok()?)?;
                if !row.is_container {
                    return None;
                }
                let mut location = state.location.clone();
                location
                    .path
                    .push((Some(row.id.clone()), row.title.clone()));
                location.offset = 0;
                Some(location)
            });
            if let Some(location) = location {
                start(&weak.unwrap(), &tx, Action::Browse(location));
            }
        });
    }
    {
        let weak = app.as_weak();
        let tx = tx.clone();
        app.on_media_breadcrumb(move |index| {
            let location = STATE.with(|state| {
                let state = state.borrow();
                let index = usize::try_from(index).ok()?;
                if index >= state.location.path.len() {
                    return None;
                }
                let mut location = state.location.clone();
                location.path.truncate(index + 1);
                location.offset = 0;
                Some(location)
            });
            if let Some(location) = location {
                start(&weak.unwrap(), &tx, Action::Browse(location));
            }
        });
    }
    {
        let weak = app.as_weak();
        let tx = tx.clone();
        app.on_media_page(move |forward| {
            let location = STATE.with(|state| {
                let state = state.borrow();
                let mut location = state.location.clone();
                location.offset = if forward {
                    state.next?
                } else {
                    (location.offset - PAGE).max(0)
                };
                Some(location)
            });
            if let Some(location) = location {
                start(&weak.unwrap(), &tx, Action::Browse(location));
            }
        });
    }
    {
        let weak = app.as_weak();
        let tx = tx.clone();
        app.on_media_play(move |index| queue(&weak.unwrap(), &tx, index, true));
    }
    {
        let weak = app.as_weak();
        let tx = tx.clone();
        app.on_media_enqueue(move |index| queue(&weak.unwrap(), &tx, index, false));
    }
    {
        let weak = app.as_weak();
        app.on_media_cancel(move || {
            let cancelled = STATE.with(|state| {
                let mut state = state.borrow_mut();
                if !state
                    .active
                    .as_ref()
                    .is_some_and(|ticket| ticket.transition_pending(RequestPhase::Cancelled))
                {
                    return false;
                }
                state.active = None;
                true
            });
            let app = weak.unwrap();
            if !cancelled {
                app.set_media_message("The queue request has already been sent.".into());
                return;
            }
            app.set_media_loading(false);
            app.set_media_busy(false);
            app.set_media_cancellable(false);
            app.set_media_message("Cancelled.".into());
        });
    }
    invalidate(app);
}

fn queue(app: &AppWindow, tx: &UnboundedSender<Cmd>, index: i32, play: bool) {
    let action = STATE.with(|state| {
        let state = state.borrow();
        let entry = state.rows.get(usize::try_from(index).ok()?)?.clone();
        if (entry.is_container && play) || (!entry.is_container && entry.track.is_none()) {
            return None;
        }
        Some(Action::Queue {
            server: state.location.server.clone()?,
            entry,
            play,
        })
    });
    if let Some(action) = action {
        start(app, tx, action);
    }
}

async fn fetch(channel: &Channel, action: Action) -> Result<Reply, Status> {
    let mut library = client(channel);
    match action {
        Action::Browse(mut location) => {
            let browser = library
                .get_media_browser(GetMediaBrowserRequest {})
                .await?
                .into_inner()
                .browser
                .ok_or_else(|| {
                    Status::failed_precondition(
                        "Connect an Emby server to browse its media libraries.",
                    )
                })?;
            if !browser.supported {
                return Err(Status::failed_precondition(
                    "This server does not support media library navigation.",
                ));
            }
            if location
                .server
                .as_ref()
                .is_some_and(|id| id != &browser.server_id)
            {
                return Err(Status::failed_precondition(
                    "The browsing server changed. Refresh its libraries.",
                ));
            }
            let response = library
                .browse_media(BrowseMediaRequest {
                    server_id: browser.server_id.clone(),
                    parent: location.path.last().and_then(|(id, _)| id.clone()),
                    offset: location.offset,
                    limit: Some(PAGE),
                })
                .await?
                .into_inner();
            if response.server_id != browser.server_id {
                return Err(Status::failed_precondition(
                    "The browsing server changed. Refresh its libraries.",
                ));
            }
            location.server = Some(response.server_id);
            Ok(Reply::Page {
                location,
                entries: response.entries,
                next: response.next_offset,
            })
        }
        Action::Queue {
            server,
            entry,
            play,
        } => {
            let tracks = if entry.is_container {
                let response = library
                    .get_media_container_tracks(GetMediaContainerTracksRequest {
                        server_id: server.clone(),
                        parent: entry.id,
                        offset: 0,
                        limit: Some(0),
                    })
                    .await?
                    .into_inner();
                if response.server_id != server {
                    return Err(Status::failed_precondition(
                        "The browsing server changed. Refresh its libraries.",
                    ));
                }
                response.tracks
            } else {
                vec![entry
                    .track
                    .ok_or_else(|| Status::failed_precondition("This item has no audio track."))?]
            };
            Ok(Reply::Queue {
                server,
                tracks,
                play,
            })
        }
    }
}

/// Runs as a separate task so a slow container read cannot block pause/stop.
pub async fn run(channel: Channel, weak: Weak<AppWindow>, request: Request) {
    let Request { ticket, action, tx } = request;
    let result = tokio::select! {
        biased;
        _ = ticket.cancelled() => return,
        result = tokio::time::timeout(Duration::from_secs(180), fetch(&channel, action)) => {
            result.unwrap_or_else(|_| Err(Status::deadline_exceeded("The media request timed out. Retry or choose a smaller container.")))
        }
    };
    let _ = weak.upgrade_in_event_loop(move |app| {
        // Recheck on the UI thread: cancellation may follow network completion
        // but precede this callback (including A -> B -> A).
        if !ticket.current() {
            return;
        }
        app.set_media_loading(false);
        match result {
            Ok(Reply::Page {
                location,
                entries,
                next,
            }) => {
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    state.location = location;
                    state.rows = entries;
                    state.next = next;
                    render(&app, &state);
                    app.set_media_message(if state.rows.is_empty() {
                        "No media in this folder.".into()
                    } else {
                        "".into()
                    });
                });
                app.set_media_busy(false);
                app.set_media_cancellable(false);
            }
            Ok(Reply::Queue {
                server,
                tracks,
                play,
            }) => {
                if tracks.is_empty() {
                    app.set_media_busy(false);
                    app.set_media_cancellable(false);
                    app.set_media_message("This container has no audio tracks.".into());
                } else if tx
                    .send(Cmd::MediaCommit(Commit {
                        ticket,
                        server,
                        tracks,
                        play,
                    }))
                    .is_err()
                {
                    app.set_media_busy(false);
                    app.set_media_cancellable(false);
                    app.set_media_message("The connection worker is unavailable.".into());
                }
            }
            Err(error) => {
                app.set_media_busy(false);
                app.set_media_cancellable(false);
                app.set_media_message(message(error).into());
            }
        }
    });
}

/// Only this command mutates the queue, after the complete read succeeded.
/// Once the mutation is sent, cancellation cannot roll it back.
pub async fn commit(channel: &Channel, weak: &Weak<AppWindow>, request: Commit) {
    let Commit {
        ticket,
        server,
        tracks,
        play,
    } = request;
    let result = async {
        let mut library = client(channel);
        let browser = tokio::select! {
            biased;
            _ = ticket.cancelled() => return Err(Status::cancelled("Cancelled")),
            result = tokio::time::timeout(Duration::from_secs(15), library.get_media_browser(GetMediaBrowserRequest {})) => {
                result.map_err(|_| Status::deadline_exceeded("Checking the connected server timed out."))??.into_inner().browser
            }
        };
        if browser.as_ref().map(|value| value.server_id.as_str()) != Some(server.as_str()) {
            return Err(Status::failed_precondition("The browsing server changed. Refresh its libraries."));
        }
        if !ticket.transition_pending(RequestPhase::Submitted) { return Err(Status::cancelled("Cancelled")); }
        let submitted = ticket.clone();
        let _ = weak.upgrade_in_event_loop(move |app| {
            if submitted.current() { app.set_media_cancellable(false); }
        });
        let mut tracklist = TracklistServiceClient::new(channel.clone());
        let mutation = async {
            if play {
                tracklist.load_tracks(LoadTracksRequest { tracks, start_index: 0 }).await?;
            } else {
                tracklist.add_tracks(AddTracksRequest { tracks }).await?;
            }
            Ok::<(), Status>(())
        };
        tokio::time::timeout(Duration::from_secs(15), mutation).await
            .map_err(|_| Status::deadline_exceeded("The queue request timed out; its result is unknown. Check the queue before trying again."))?
    }.await;
    let _ = weak.upgrade_in_event_loop(move |app| {
        if !ticket.current() {
            return;
        }
        app.set_media_busy(false);
        app.set_media_cancellable(false);
        app.set_media_message(match result {
            Ok(()) => {
                if play {
                    "Playback requested.".into()
                } else {
                    "Added to queue.".into()
                }
            }
            Err(error) => message(error).into(),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_before_or_during_subscription_wakes_and_never_reactivates() {
        let old_a = Ticket::new();
        old_a.cancel();
        tokio::time::timeout(Duration::from_secs(1), old_a.cancelled())
            .await
            .unwrap();
        let b = Ticket::new();
        let waiting = b.clone();
        let pending = tokio::spawn(async move { waiting.cancelled().await });
        b.cancel();
        tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap();
        let new_a = Ticket::new();
        old_a.cancel();
        assert!(!old_a.current());
        assert!(!b.current());
        assert!(new_a.current());
    }

    #[test]
    fn cancellation_and_submission_are_exclusive_but_a_source_switch_invalidates_both() {
        let cancelled = Ticket::new();
        assert!(cancelled.transition_pending(RequestPhase::Cancelled));
        assert!(!cancelled.transition_pending(RequestPhase::Submitted));
        let submitted = Ticket::new();
        assert!(submitted.transition_pending(RequestPhase::Submitted));
        assert!(!submitted.transition_pending(RequestPhase::Cancelled));
        assert!(!submitted.transition_pending(RequestPhase::Submitted));
        assert!(submitted.current());
        submitted.cancel();
        assert!(!submitted.current());
    }

    #[test]
    fn media_indices_are_exact_and_do_not_change_the_original_title() {
        let entry = MediaEntry {
            title: "My First Ever Ender Dragon Fight! [First Ever Minecraft Playthrough Ep.40]"
                .into(),
            item_type: Some("Episode".into()),
            season_number: Some(u64::MAX),
            episode_number: Some(0),
            track: Some(Track::default()),
            ..Default::default()
        };
        assert_eq!(subtitle(&entry), "Episode · S18446744073709551615 · E0");
        assert!(entry.title.ends_with("Ep.40]"));
    }
}
