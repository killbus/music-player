use async_graphql::*;
use music_player_playback::player::{PlayerRuntime, PlayerRuntimePhase};

#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum PlayerRuntimePhaseType {
    #[graphql(name = "Starting")]
    Starting,
    #[graphql(name = "Ready")]
    Ready,
    #[graphql(name = "Failed")]
    Failed,
}

impl From<PlayerRuntimePhase> for PlayerRuntimePhaseType {
    fn from(phase: PlayerRuntimePhase) -> Self {
        match phase {
            PlayerRuntimePhase::Starting => Self::Starting,
            PlayerRuntimePhase::Ready => Self::Ready,
            PlayerRuntimePhase::Failed => Self::Failed,
        }
    }
}

#[derive(Clone)]
pub struct PlayerRuntimeType {
    snapshot: music_player_playback::player::PlayerRuntimeSnapshot,
}

#[Object]
impl PlayerRuntimeType {
    async fn output(&self) -> &str {
        &self.snapshot.output
    }

    async fn phase(&self) -> PlayerRuntimePhaseType {
        self.snapshot.phase.into()
    }

    async fn error(&self) -> Option<&str> {
        self.snapshot.error.as_deref()
    }
}

impl From<music_player_playback::player::PlayerRuntimeSnapshot> for PlayerRuntimeType {
    fn from(snapshot: music_player_playback::player::PlayerRuntimeSnapshot) -> Self {
        Self { snapshot }
    }
}

pub fn player_runtime(runtime: &PlayerRuntime) -> PlayerRuntimeType {
    runtime.lock().unwrap().clone().into()
}
