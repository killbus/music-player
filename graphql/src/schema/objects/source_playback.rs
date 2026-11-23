use async_graphql::{Enum, SimpleObject};
use music_player_playback::{managed::Phase, player::SourcePlaybackStatus};

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum SourcePlaybackPhase {
    Stopped,
    Paused,
    Resolving,
    Opening,
    Recovering,
    Streaming,
    Draining,
    Failed,
    EndUnconfirmed,
    Ended,
}

#[derive(SimpleObject)]
pub struct SourcePlayback {
    pub source: String,
    pub occurrence_id: String,
    pub phase: SourcePlaybackPhase,
    pub error: Option<String>,
    pub retry_attempt: u32,
    pub retry_limit: u32,
}

impl From<SourcePlaybackStatus> for SourcePlayback {
    fn from(value: SourcePlaybackStatus) -> Self {
        Self {
            source: value.source,
            occurrence_id: value.occurrence_id,
            phase: match value.phase {
                Phase::Stopped => SourcePlaybackPhase::Stopped,
                Phase::Paused => SourcePlaybackPhase::Paused,
                Phase::Resolving => SourcePlaybackPhase::Resolving,
                Phase::Opening => SourcePlaybackPhase::Opening,
                Phase::Recovering => SourcePlaybackPhase::Recovering,
                Phase::Streaming => SourcePlaybackPhase::Streaming,
                Phase::Draining => SourcePlaybackPhase::Draining,
                Phase::Failed => SourcePlaybackPhase::Failed,
                Phase::EndUnconfirmed => SourcePlaybackPhase::EndUnconfirmed,
                Phase::Ended => SourcePlaybackPhase::Ended,
            },
            error: value.error,
            retry_attempt: value.retry_attempt,
            retry_limit: value.retry_limit,
        }
    }
}
