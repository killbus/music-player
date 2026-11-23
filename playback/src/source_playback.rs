//! Player-thread adapter for account-bound audio. Resolution never uses the
//! currently browsed provider, and no transient URL enters the engine queue.
use crate::{managed::*, source_descriptor::SourceDescriptor, source_resolver::SourceResolver};
use music_player_provider::ProviderError;
use music_player_transport::{HttpReader, HttpRequest, TransportHandle, TransportTerminal};
use music_player_types::audio::AudioSelection;
use music_player_types::source::{ResourceKind, SourceHandle};
use rockbox_playback::{Metadata, Player as Engine, StreamSession};
use std::time::{Duration, Instant};
use tokio::{sync::mpsc, task::JoinHandle};

mod recovery;
use recovery::{Recovery, RETRY_LIMIT};

#[derive(Clone)]
struct Occurrence {
    id: String,
    source: SourceHandle,
}
type Completion = (
    ResolveRequest<Occurrence>,
    Result<SourceDescriptor, ProviderError>,
);

/// Current source attempt, independent of output initialization. Contains no
/// temporary media URLs, request headers or credentials.
#[derive(Clone, Debug)]
pub struct SourcePlaybackStatus {
    pub source: String,
    pub occurrence_id: String,
    pub phase: Phase,
    pub error: Option<String>,
    pub retry_attempt: u32,
    pub retry_limit: u32,
}

struct StreamObservation {
    session: StreamSession,
    transport: TransportHandle,
}

/// No URL, headers or session token. Offset is an output delivery estimate.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SourceCheckpoint {
    pub(crate) version: u32,
    pub(crate) occurrence_id: String,
    pub(crate) source: String,
    pub(crate) offset_ms: u64,
    pub(crate) selection: AudioSelection,
    /// None means no descriptor has been accepted, not that seeking is disabled.
    #[serde(default)]
    pub(crate) capabilities: Option<PlaybackCapabilities>,
}

impl SourceCheckpoint {
    pub(crate) fn capabilities_for(
        &self,
        source: &SourceHandle,
    ) -> Result<Option<PlaybackCapabilities>, &'static str> {
        match self.version {
            1 if source.resolver() == "emby" && self.capabilities.is_none() => {
                // The old checkpoint format was emitted only by native Emby.
                // Unknown duration never removes its server-offset capability.
                let timeline = match &self.selection {
                    AudioSelection::Pinned(pin) => pin
                        .runtime_ticks
                        .map_or(Timeline::Unknown, |duration_ticks| Timeline::Finite {
                            duration_ticks,
                        }),
                    _ => Timeline::Unknown,
                };
                Ok(Some(PlaybackCapabilities {
                    timeline,
                    seek_policy: SeekPolicy::ServerOffset,
                }))
            }
            2 if matches!(source, SourceHandle::Local(_)) => {
                if self.selection != AudioSelection::Auto
                    || self
                        .capabilities
                        .is_some_and(|value| value != legacy_capabilities())
                {
                    return Err("source v1 checkpoint has incompatible audio capabilities");
                }
                Ok(Some(legacy_capabilities()))
            }
            2 => Ok(self.capabilities),
            _ => Err("unsupported audio checkpoint capabilities"),
        }
    }
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
    observation: Option<StreamObservation>,
    recovery: Recovery,
    retryable: bool,
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
            observation: None,
            recovery: Recovery::default(),
            retryable: false,
        }
    }
}
impl<S: SessionControl> SourcePlayback<S> {
    pub fn is_loaded(&self) -> bool {
        self.occurrence.is_some()
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.coordinator.snapshot();
        if self.recovery.active {
            snapshot.phase = Phase::Recovering;
        }
        snapshot
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn status(&self) -> Option<SourcePlaybackStatus> {
        let occurrence = self.occurrence.as_ref()?;
        Some(SourcePlaybackStatus {
            source: occurrence.source.to_handle(),
            occurrence_id: occurrence.id.clone(),
            phase: self.snapshot().phase,
            error: if self.recovery.active { None } else { self.error.clone() },
            retry_attempt: self.recovery.attempt,
            retry_limit: RETRY_LIMIT,
        })
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
            version: 2,
            occurrence_id: occurrence.id.clone(),
            source: occurrence.source.to_handle(),
            offset_ms: u64::try_from(position.as_millis()).ok()?,
            selection: self.selection.clone(),
            capabilities: snapshot.capabilities,
        })
    }
    pub fn restore(
        &mut self,
        source: SourceHandle,
        checkpoint: SourceCheckpoint,
    ) -> Result<(), ProviderError> {
        let capabilities = checkpoint
            .capabilities_for(&source)
            .map_err(|message| ProviderError::Other(message.into()))?;
        if checkpoint.source != source.to_handle()
            || SourceHandle::parse(&source.to_handle()).is_err()
            || source.kind() != ResourceKind::Item
            || uuid::Uuid::parse_str(&checkpoint.occurrence_id).is_err()
        {
            return Err(ProviderError::Other(
                "saved audio checkpoint is incompatible with this queue entry".into(),
            ));
        }
        // Restore is entirely local and paused. Authentication is deferred
        // until Play; it then checks the saved selection and current account.
        self.cancel_task();
        self.recovery = Recovery::default();
        let occurrence = Occurrence {
            id: checkpoint.occurrence_id,
            source,
        };
        self.coordinator.load_with_capabilities(
            occurrence.clone(),
            Duration::from_millis(checkpoint.offset_ms),
            false,
            capabilities,
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
        source: SourceHandle,
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
        source: SourceHandle,
        occurrence_id: String,
        selection: AudioSelection,
        target: Duration,
        playing: bool,
    ) -> Result<(), ProviderError> {
        if SourceHandle::parse(&source.to_handle()).is_err()
            || source.kind() != ResourceKind::Item
            || uuid::Uuid::parse_str(&occurrence_id).is_err()
        {
            return Err(ProviderError::Other(
                "a playable source and valid queue occurrence are required".into(),
            ));
        }
        let capabilities = if matches!(source, SourceHandle::Local(_)) {
            if selection != AudioSelection::Auto || !target.is_zero() {
                return Err(ProviderError::Other(
                    "source v1 does not support offsets or audio selection".into(),
                ));
            }
            Some(legacy_capabilities())
        } else {
            None
        };
        let occurrence = Occurrence {
            id: occurrence_id,
            source,
        };
        self.selection = selection;
        self.recovery = Recovery::default();
        self.error = None;
        let request = self.coordinator.load_with_capabilities(
            occurrence.clone(),
            target,
            playing,
            capabilities,
        );
        self.occurrence = Some(occurrence);
        self.launch(request);
        Ok(())
    }
    pub fn play(&mut self) -> Result<(), ControlError> {
        // Redundant Play cannot bypass backoff or replenish its budget.
        if self.recovery.active {
            return Ok(());
        }
        let request = self.coordinator.play().inspect_err(|error| {
            self.error = Some(error.to_string());
        })?;
        // Repeated play must not abort the currently valid resolve.
        if request.is_some() {
            self.recovery = Recovery::default();
            self.launch(request);
        }
        Ok(())
    }
    pub fn pause(&mut self) {
        self.recovery = Recovery::default();
        self.coordinator.pause();
        self.cancel_task();
        self.error = None;
    }
    pub fn stop(&mut self) {
        self.recovery = Recovery::default();
        self.coordinator.stop();
        self.cancel_task();
        self.error = None;
    }
    pub fn clear(&mut self) {
        self.stop();
        self.coordinator = Coordinator::default();
        self.occurrence = None;
        self.selection = AudioSelection::Auto;
        self.error = None;
    }
    pub fn seek(&mut self, target: Duration) -> Result<(), ControlError> {
        let request = self.coordinator.seek(target).inspect_err(|error| {
            self.error = Some(error.to_string());
        })?;
        self.recovery = Recovery::default();
        self.launch(request);
        Ok(())
    }
    fn cancel_task(&mut self) {
        self.retryable = false;
        self.observation = None;
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

    fn expire_recovery(&mut self, now: Instant) -> bool {
        if !self.recovery.active || !self.recovery.expired(now) { return false; }
        self.coordinator.fail();
        self.cancel_task();
        self.recovery.finish();
        self.error = Some("Playback recovery timed out. Press Play to retry from the saved position.".into());
        true
    }

    fn recover(&mut self, now: Instant) {
        let snapshot = self.coordinator.snapshot();
        if snapshot.phase == Phase::Ended {
            self.recovery.finish();
            return;
        }
        let native_emby = self.occurrence.as_ref().is_some_and(|o| {
            matches!(o.source, SourceHandle::Remote(_)) && o.source.resolver() == "emby"
        });
        if native_emby && snapshot.phase == Phase::EndUnconfirmed
            && matches!(snapshot.capabilities.map(|c| c.timeline), Some(Timeline::Finite { .. }))
            && self.error.is_none()
        {
            self.coordinator.fail();
            self.retryable = true;
            self.error = Some("Audio ended before the expected duration. Press Play to resume.".into());
        }
        if snapshot.is_playing() {
            if let Some(position) = snapshot.position.filter(|p| Some(p.generation) == snapshot.generation) {
                self.recovery.observe(position, now);
            }
        } else if self.coordinator.snapshot().phase == Phase::Failed && self.recovery.retry_at.is_none() {
            let retryable = std::mem::take(&mut self.retryable);
            if retryable && native_emby && snapshot.desired == DesiredState::Playing {
                let jitter = Duration::from_millis(u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 251);
                if !self.recovery.schedule(now, jitter) {
                    let reason = self.error.as_deref().unwrap_or("Audio source failed");
                    self.error = Some(format!(
                        "Playback recovery attempts failed. Last failure: {reason}. Press Play to retry from the saved position."
                    ));
                }
            } else {
                self.recovery.finish();
            }
        }
        if self.recovery.retry_at.is_some_and(|when| now >= when) {
            self.recovery.retry_at = None;
            match self.coordinator.play() {
                Ok(request) => self.launch(request),
                Err(error) => {
                    self.recovery.finish();
                    self.error = Some(error.to_string());
                }
            }
        }
    }

    fn poll_with(&mut self, mut start: impl FnMut((HttpRequest, String)) -> Result<S, String>) {
        while let Ok((request, result)) = self.completed.try_recv() {
            match result {
                Err(error) => {
                    if self.coordinator.resolve_failed(&request) == ResolveOutcome::Failed {
                        // Empty audio metadata can follow an upstream probe
                        // failure. It receives the same bounded fresh attempts.
                        self.retryable = matches!(
                            error,
                            ProviderError::Transport(_) | ProviderError::Upstream(_)
                                | ProviderError::NotFound(_) | ProviderError::NoAudioStream(_)
                        );
                        self.error = Some(error.to_string());
                    }
                }
                Ok(audio) => {
                    let SourceDescriptor {
                        request: http,
                        format_ext,
                        start: origin,
                        capabilities,
                        requested_offset_ms,
                        pin,
                        lease,
                    } = audio;
                    // The requested target must match this attempt; a calibrated
                    // actual start is separate evidence and may legitimately differ.
                    // Resolver offsets use milliseconds; delivered PCM may
                    // freeze between millisecond boundaries during recovery.
                    if u128::from(requested_offset_ms) != request.target().as_millis() {
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
                            capabilities,
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
                        self.error = Some(start_error.unwrap_or_else(|| {
                            "resolved source capabilities do not match this playback attempt".into()
                        }));
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
        let now = Instant::now();
        if self.expire_recovery(now) { return; }
        let mut observation = None;
        self.poll_with(|(http, format_ext)| {
            let (reader, transport) = HttpReader::start(http)
                .map_err(|_| "could not start audio transport".to_owned())?;
            let cancellation = transport.clone();
            let session = engine.play_stream(
                Box::new(reader),
                format_ext,
                Metadata::default(),
                move || cancellation.cancel(),
            );
            observation = Some(StreamObservation {
                session: session.clone(),
                transport,
            });
            Ok(session)
        });
        if observation.is_some() {
            self.observation = observation;
        }
        let snapshot = self.coordinator.snapshot();
        if snapshot.phase == Phase::Failed && self.error.is_none() {
            self.retryable = self.observation.as_ref().is_some_and(|observed| {
                // An output accounting invariant is local; only a failed input
                // or decoder is evidence for another upstream attempt.
                observed.session.snapshot().phase == rockbox_playback::StreamPhase::Failed
                    && retry_stream(&observed.transport.snapshot().terminal)
            });
            self.error = Some(match &self.observation {
                Some(observed) => stream_failure(
                    &observed.transport.snapshot().terminal,
                    observed.session.snapshot().decoder_status,
                ),
                None => "Audio stream failed. Press Play to retry from the saved position.".into(),
            });
        }
        self.recover(now);
    }
}

fn retry_stream(terminal: &TransportTerminal) -> bool {
    // Fresh resolution can replace an unusable media response, including an
    // expired URL or a decoder failure under HTTP 200. Do not infer its cause.
    // Rate limiting needs Retry-After support. A Cancelled transport here may
    // be cleanup after decoder failure; user cancellation is checked by phase.
    !matches!(terminal, TransportTerminal::HttpStatus(429))
}

fn stream_failure(terminal: &TransportTerminal, decoder_status: Option<i32>) -> String {
    match terminal {
        TransportTerminal::HttpStatus(status) => {
            format!("Audio server returned HTTP {status}. Press Play to resolve a fresh stream.")
        }
        TransportTerminal::HeaderTimeout => {
            "Audio server did not respond in time. Press Play to retry.".into()
        }
        TransportTerminal::ReadTimeout => {
            "Audio stream stopped sending data. Press Play to resume.".into()
        }
        TransportTerminal::NetworkError => {
            "Audio stream connection failed. Press Play to resume.".into()
        }
        TransportTerminal::RedirectLimit => "Audio stream exceeded the redirect limit.".into(),
        TransportTerminal::InvalidRedirect => "Audio server returned an invalid redirect.".into(),
        _ => match decoder_status {
            Some(code) if code != 0 => {
                format!("Audio decoder failed (code {code}). Press Play to retry.")
            }
            _ => "Audio could not be decoded or delivered. Press Play to retry.".into(),
        },
    }
}

fn legacy_capabilities() -> PlaybackCapabilities {
    PlaybackCapabilities {
        timeline: Timeline::Unknown,
        seek_policy: SeekPolicy::None,
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

#[cfg(test)]
mod extension_http_tests;
