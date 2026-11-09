use async_graphql::{Context, Enum, Error, SimpleObject};
use music_player_playback::player::PlayerCommand;
use music_player_renderer::CurrentReceiverDevice;
use music_player_tracklist::Tracklist;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{mpsc::UnboundedSender, oneshot, Mutex};

use crate::require_local_player_ready;

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn value(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::All => 1,
            Self::One => 2,
        }
    }
}

#[derive(SimpleObject)]
pub struct PlaybackModes {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub shuffle: Option<bool>,
    pub repeat: Option<RepeatMode>,
}

const REMOTE_UNAVAILABLE: &str = "Playback modes are unavailable on this remote receiver";

pub async fn read_modes(ctx: &Context<'_>) -> Result<PlaybackModes, Error> {
    let device = ctx
        .data::<Arc<Mutex<CurrentReceiverDevice>>>()?
        .lock()
        .await;
    let unavailable_reason = if device.client.is_some() || device.receiver_device.is_some() {
        Some(REMOTE_UNAVAILABLE.to_string())
    } else {
        require_local_player_ready(ctx).err().map(|err| err.message)
    };
    if unavailable_reason.is_some() {
        return Ok(PlaybackModes {
            available: false,
            unavailable_reason,
            shuffle: None,
            repeat: None,
        });
    }
    let queue = ctx.data::<Arc<StdMutex<Tracklist>>>()?.lock().unwrap();
    Ok(PlaybackModes {
        available: true,
        unavailable_reason: None,
        shuffle: Some(queue.shuffle_enabled()),
        repeat: Some(match queue.repeat_mode() {
            1 => RepeatMode::All,
            2 => RepeatMode::One,
            _ => RepeatMode::Off,
        }),
    })
}

pub async fn set_modes(
    ctx: &Context<'_>,
    shuffle: Option<bool>,
    repeat: Option<RepeatMode>,
) -> Result<bool, Error> {
    // Keep routing stable until the local player has applied the command.
    let device = ctx
        .data::<Arc<Mutex<CurrentReceiverDevice>>>()?
        .lock()
        .await;
    if device.client.is_some() || device.receiver_device.is_some() {
        return Err(Error::new(REMOTE_UNAVAILABLE));
    }
    require_local_player_ready(ctx)?;
    let (reply, received) = oneshot::channel();
    ctx.data::<Arc<StdMutex<UnboundedSender<PlayerCommand>>>>()?
        .lock()
        .unwrap()
        .send(PlayerCommand::SetPlaybackModes {
            shuffle,
            repeat_mode: repeat.map(RepeatMode::value),
            reply,
        })
        .map_err(|_| Error::new("Player command channel closed"))?;
    tokio::time::timeout(std::time::Duration::from_secs(5), received)
        .await
        .map_err(|_| Error::new("Playback mode acknowledgement timed out; outcome uncertain. Refresh to check the current mode."))?
        .map_err(|_| Error::new("Playback mode acknowledgement channel closed; refresh to check the current mode."))?
        .map_err(Error::new)?;
    Ok(true)
}
