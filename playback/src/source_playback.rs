//! Player-thread adapter for account-bound audio. Resolution never uses the
//! currently browsed provider, and no transient URL enters the engine queue.
use crate::{managed::*, source_descriptor::SourceDescriptor, source_resolver::SourceResolver};
use music_player_provider::ProviderError;
use music_player_transport::{HttpReader, HttpRequest};
use music_player_types::audio::AudioSelection;
use music_player_types::source::{ResourceKind, SourceRef};
use rockbox_playback::{Metadata, Player as Engine, StreamSession};
use std::time::Duration;
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Clone)]
struct Occurrence {
    id: String,
    source: SourceRef,
}
type Completion = (
    ResolveRequest<Occurrence>,
    Result<SourceDescriptor, ProviderError>,
);

/// No URL, headers or session token. Offset is an output delivery estimate.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SourceCheckpoint {
    pub(crate) version: u32,
    pub(crate) occurrence_id: String,
    pub(crate) source: String,
    pub(crate) offset_ms: u64,
    pub(crate) selection: AudioSelection,
}

pub(crate) struct SourcePlayback<S: SessionControl = StreamSession> {
    resolver: SourceResolver,
    coordinator: Coordinator<Occurrence, S>,
    occurrence: Option<Occurrence>,
    selection: AudioSelection,
    task: Option<JoinHandle<()>>,
    completed: mpsc::Receiver<Completion>,
    sender: mpsc::Sender<Completion>,
    error: Option<String>,
}
impl SourcePlayback {
    pub fn new(resolver: SourceResolver) -> Self {
        let (sender, completed) = mpsc::channel(1);
        Self {
            resolver,
            coordinator: Coordinator::default(),
            occurrence: None,
            selection: AudioSelection::Auto,
            task: None,
            completed,
            sender,
            error: None,
        }
    }
}
impl<S: SessionControl> SourcePlayback<S> {
    pub fn is_loaded(&self) -> bool {
        self.occurrence.is_some()
    }
    pub fn snapshot(&self) -> Snapshot {
        self.coordinator.snapshot()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn accepted_pin(&self) -> Option<(&str, &music_player_types::audio::AudioPin)> {
        let AudioSelection::Pinned(pin) = &self.selection else {
            return None;
        };
        Some((&self.occurrence.as_ref()?.id, pin))
    }
    pub fn checkpoint(&self) -> Option<SourceCheckpoint> {
        let occurrence = self.occurrence.as_ref()?;
        let snapshot = self.snapshot();
        let position = snapshot
            .position
            .filter(|p| Some(p.generation) == snapshot.generation)
            .map(|p| p.absolute)
            .unwrap_or(snapshot.target);
        Some(SourceCheckpoint {
            version: 1,
            occurrence_id: occurrence.id.clone(),
            source: occurrence.source.to_handle(),
            offset_ms: u64::try_from(position.as_millis()).ok()?,
            selection: self.selection.clone(),
        })
    }
    pub fn restore(
        &mut self,
        source: SourceRef,
        checkpoint: SourceCheckpoint,
    ) -> Result<(), ProviderError> {
        if checkpoint.version != 1
            || checkpoint.source != source.to_handle()
            || source.kind != ResourceKind::Item
            || uuid::Uuid::parse_str(&checkpoint.occurrence_id).is_err()
        {
            return Err(ProviderError::Other(
                "saved audio checkpoint is incompatible with this queue entry".into(),
            ));
        }
        // Restore is entirely local and paused. Authentication is deferred
        // until Play; it then checks the saved selection and current account.
        self.cancel_task();
        let occurrence = Occurrence {
            id: checkpoint.occurrence_id,
            source,
        };
        self.coordinator.load(
            occurrence.clone(),
            Duration::from_millis(checkpoint.offset_ms),
            false,
        );
        self.occurrence = Some(occurrence);
        // Before a first accepted resolution, a paused seek is still selection
        // intent. After acceptance, retries/restores retain the exact pin.
        self.selection = checkpoint.selection;
        self.error = None;
        Ok(())
    }

    pub fn load(
        &mut self,
        source: SourceRef,
        target: Duration,
        playing: bool,
    ) -> Result<(), ProviderError> {
        self.load_occurrence(
            source,
            uuid::Uuid::new_v4().to_string(),
            AudioSelection::Auto,
            target,
            playing,
        )
    }

    pub fn load_occurrence(
        &mut self,
        source: SourceRef,
        occurrence_id: String,
        selection: AudioSelection,
        target: Duration,
        playing: bool,
    ) -> Result<(), ProviderError> {
        if source.kind != ResourceKind::Item || uuid::Uuid::parse_str(&occurrence_id).is_err() {
            return Err(ProviderError::Other(
                "a playable source and valid queue occurrence are required".into(),
            ));
        }
        let occurrence = Occurrence {
            id: occurrence_id,
            source,
        };
        self.selection = selection;
        self.error = None;
        let request = self.coordinator.load(occurrence.clone(), target, playing);
        self.occurrence = Some(occurrence);
        self.launch(request);
        Ok(())
    }
    pub fn play(&mut self) {
        let request = self.coordinator.play();
        // Repeated play must not abort the currently valid resolve.
        if request.is_some() {
            self.launch(request);
        }
    }
    pub fn pause(&mut self) {
        self.coordinator.pause();
        self.cancel_task();
    }
    pub fn stop(&mut self) {
        self.coordinator.stop();
        self.cancel_task();
    }
    pub fn clear(&mut self) {
        self.stop();
        self.coordinator = Coordinator::default();
        self.occurrence = None;
        self.selection = AudioSelection::Auto;
        self.error = None;
    }
    pub fn seek(&mut self, target: Duration) {
        let request = self.coordinator.seek(target);
        self.launch(request);
    }
    fn cancel_task(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        // Dropping queued stale results also drops their owned encoding lease.
        while self.completed.try_recv().is_ok() {}
    }
    fn launch(&mut self, request: Option<ResolveRequest<Occurrence>>) {
        self.cancel_task();
        let Some(request) = request else { return };
        self.error = None;
        let resolver = self.resolver.clone();
        let selection = self.selection.clone();
        let sender = self.sender.clone();
        self.task = Some(tokio::spawn(async move {
            let offset = match u64::try_from(request.target().as_millis()) {
                Ok(offset) => offset,
                Err(_) => {
                    let _ = sender
                        .send((
                            request,
                            Err(ProviderError::Other("seek offset is out of range".into())),
                        ))
                        .await;
                    return;
                }
            };
            let result = tokio::select! {
                biased;
                _ = request.cancelled() => return,
                result = resolver.resolve_playback(&request.key().source, &selection, offset) => result,
            };
            // A full mailbox never accumulates unbounded resolved sessions.
            tokio::select! {
                biased;
                _ = request.cancelled() => {},
                _ = sender.send((request.clone(), result)) => {},
            }
        }));
    }

    fn poll_with(&mut self, mut start: impl FnMut((HttpRequest, String)) -> Result<S, String>) {
        while let Ok((request, result)) = self.completed.try_recv() {
            match result {
                Err(error) => {
                    if self.coordinator.resolve_failed(&request) == ResolveOutcome::Failed {
                        self.error = Some(error.to_string());
                    }
                }
                Ok(audio) => {
                    let SourceDescriptor {
                        request: http,
                        format_ext,
                        start: origin,
                        requested_offset_ms,
                        pin,
                        lease,
                    } = audio;
                    // The requested target must match this attempt; a calibrated
                    // actual start is separate evidence and may legitimately differ.
                    if Duration::from_millis(requested_offset_ms) != request.target() {
                        if self.coordinator.resolve_failed(&request) == ResolveOutcome::Failed {
                            self.error =
                                Some("resolved source target does not match the request".into());
                        }
                        continue;
                    }
                    let mut start_error = None;
                    let outcome = self.coordinator.resolved(
                        &request,
                        Resolved {
                            payload: (http, format_ext),
                            start: origin,
                            lease,
                        },
                        |http| {
                            start(http).map_err(|error| {
                                start_error = Some(error);
                            })
                        },
                    );
                    if outcome == ResolveOutcome::Started {
                        // Each new load has a distinct occurrence, even for the
                        // same item. Only its accepted result can fix its pin.
                        if self
                            .occurrence
                            .as_ref()
                            .is_some_and(|o| o.id == request.key().id)
                        {
                            if let Some(pin) = pin {
                                self.selection = AudioSelection::Pinned(pin);
                            }
                        }
                    } else if outcome == ResolveOutcome::Failed {
                        self.error = start_error;
                    }
                }
            }
        }
        self.coordinator.observe();
    }
}
impl SourcePlayback {
    /// Called on the same thread as player commands. The transport is created
    /// inside the acceptance callback, never before the ticket is validated.
    pub fn poll(&mut self, engine: &Engine) {
        self.poll_with(|(http, format_ext)| {
            let (reader, transport) = HttpReader::start(http)
                .map_err(|_| "could not start audio transport".to_owned())?;
            Ok(engine.play_stream(
                Box::new(reader),
                format_ext,
                Metadata::default(),
                move || transport.cancel(),
            ))
        });
    }
}
impl<S: SessionControl> Drop for SourcePlayback<S> {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod http_tests;
