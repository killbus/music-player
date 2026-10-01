//! In-memory bridge from source resolution to the managed playback host.
//! Contains transient credentials; deliberately neither Debug nor serializable.
use crate::managed::{PlaybackLease, StartPosition};
use music_player_provider::emby_playback::ResolvedAudio;
use music_player_transport::HttpRequest;
use music_player_types::audio::AudioPin;

pub(crate) struct SourceDescriptor {
    pub(crate) request: HttpRequest,
    /// Decoder format extension hint, not a URL suffix or MIME type.
    pub(crate) format_ext: String,
    pub(crate) start: StartPosition,
    pub(crate) requested_offset_ms: u64,
    pub(crate) pin: Option<AudioPin>,
    pub(crate) lease: PlaybackLease,
}

impl From<ResolvedAudio> for SourceDescriptor {
    fn from(audio: ResolvedAudio) -> Self {
        let ResolvedAudio {
            url,
            headers,
            follow_redirects,
            pin,
            requested_offset_ms,
            lease,
        } = audio;
        let mut request = HttpRequest::new(url.into());
        request.headers = headers;
        request.follow_redirects = follow_redirects;
        Self {
            request,
            format_ext: "mp3".into(),
            start: StartPosition::RequestedOnly,
            requested_offset_ms,
            pin: Some(pin),
            // EncodingLease Drop schedules bounded native cleanup without
            // waiting for network I/O on the player command loop.
            lease: PlaybackLease::new(move || drop(lease)),
        }
    }
}
