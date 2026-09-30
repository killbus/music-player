use std::{collections::HashSet, sync::Arc};

use music_player_entity::{album, artist, track};
use music_player_playback::{
    player::{normalize_queue, PlayerCommand},
    source_resolver::SourceResolver,
};
use music_player_storage::Database;
use music_player_tracklist::Tracklist as TracklistState;
use music_player_types::source::SourceRef;
use sea_orm::EntityTrait;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{
    metadata::v1alpha1::Track,
    music::v1alpha1::{
        tracklist_service_server::TracklistService, AddTrackRequest, AddTrackResponse,
        AddTracksRequest, AddTracksResponse, ClearTracklistRequest, ClearTracklistResponse,
        FilterTracklistRequest, FilterTracklistResponse, GetNextTrackRequest, GetNextTrackResponse,
        GetPreviousTrackRequest, GetPreviousTrackResponse, GetRandomRequest, GetRandomResponse,
        GetRepeatRequest, GetRepeatResponse, GetSingleRequest, GetSingleResponse,
        GetTracklistTracksRequest, GetTracklistTracksResponse, LoadTracksRequest,
        LoadTracksResponse, PlayNextRequest, PlayNextResponse, PlayTrackAtRequest,
        PlayTrackAtResponse, RemoveTrackAtRequest, RemoveTrackAtResponse, SetRepeatRequest,
        SetRepeatResponse, ShuffleRequest, ShuffleResponse,
    },
};

pub struct Tracklist {
    state: Arc<std::sync::Mutex<TracklistState>>,
    cmd_tx: Arc<std::sync::Mutex<UnboundedSender<PlayerCommand>>>,
    db: Database,
}

impl Tracklist {
    pub fn new(
        state: Arc<std::sync::Mutex<TracklistState>>,
        cmd_tx: Arc<std::sync::Mutex<UnboundedSender<PlayerCommand>>>,
        db: Database,
    ) -> Self {
        Self { state, cmd_tx, db }
    }

    // Validate every entry before any command. Full metadata batches need only
    // a saved-account check; the player reauthenticates when opening a source.
    async fn validate_queue(&self, tracks: &mut [track::Model]) -> Result<(), tonic::Status> {
        normalize_queue(tracks).map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        let mut checked = HashSet::new();
        for track in tracks {
            if SourceRef::is_handle(&track.id) {
                let source = SourceRef::parse(&track.id)
                    .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
                if checked.insert((source.account_id.clone(), source.remote.clone())) {
                    SourceResolver::from_settings(self.db.clone())
                        .validate_saved(&source)
                        .await
                        .map_err(|e| tonic::Status::failed_precondition(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    async fn resolve_source_track(&self, track: &mut track::Model) -> Result<bool, tonic::Status> {
        normalize_queue(std::slice::from_mut(track))
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        if !SourceRef::is_handle(&track.id) {
            return Ok(false);
        }
        let source = SourceRef::parse(&track.id)
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        *track = SourceResolver::from_settings(self.db.clone())
            .track(&source)
            .await
            .map_err(|e| tonic::Status::failed_precondition(e.to_string()))?
            .into();
        Ok(true)
    }
}

#[tonic::async_trait]
impl TracklistService for Tracklist {
    async fn add_track(
        &self,
        request: tonic::Request<AddTrackRequest>,
    ) -> Result<tonic::Response<AddTrackResponse>, tonic::Status> {
        let song = request
            .into_inner()
            .track
            .ok_or_else(|| tonic::Status::invalid_argument("Track is required"))?;
        let mut song: track::Model = song.into();
        if self.resolve_source_track(&mut song).await? {
            self.cmd_tx
                .lock()
                .unwrap()
                .send(PlayerCommand::LoadTracklist {
                    tracks: vec![song],
                    start_index: None,
                })
                .map_err(|_| tonic::Status::unavailable("Player command channel closed"))?;
            return Ok(tonic::Response::new(AddTrackResponse {}));
        }
        let id = song.id;

        let result: Vec<(track::Model, Vec<artist::Model>)> = track::Entity::find_by_id(id.clone())
            .find_with_related(artist::Entity)
            .all(self.db.get_connection())
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
        if result.is_empty() {
            return Err(tonic::Status::not_found("Track not found"));
        }

        let (mut track, artists) = result.into_iter().next().unwrap();
        track.artists = artists;

        let result: Vec<(track::Model, Option<album::Model>)> =
            track::Entity::find_by_id(id.clone())
                .find_also_related(album::Entity)
                .all(self.db.get_connection())
                .await
                .map_err(|e| tonic::Status::internal(e.to_string()))?;
        let (_, album) = result.into_iter().next().unwrap();
        track.album = album.unwrap_or_default();
        self.validate_queue(std::slice::from_mut(&mut track))
            .await?;

        self.cmd_tx
            .lock()
            .unwrap()
            .send(PlayerCommand::LoadTracklist {
                tracks: vec![track],
                // Appending one track: wherever playback is, it stays.
                start_index: None,
            })
            .unwrap();
        let response = AddTrackResponse {};
        Ok(tonic::Response::new(response))
    }

    /// Append tracks to the queue.
    ///
    /// Takes whole tracks rather than ids, unlike `AddTrack`. Ids would have to
    /// be looked up here, against a local table that holds nothing when a
    /// remote provider is connected — so the caller sends what it already has
    /// from the listing it is adding from, and this works whatever the library
    /// is.
    async fn add_tracks(
        &self,
        request: tonic::Request<AddTracksRequest>,
    ) -> Result<tonic::Response<AddTracksResponse>, tonic::Status> {
        let mut tracks = request
            .into_inner()
            .tracks
            .into_iter()
            .map(Into::into)
            .collect::<Vec<track::Model>>();

        self.validate_queue(&mut tracks).await?;

        if !tracks.is_empty() {
            self.cmd_tx
                .lock()
                .unwrap()
                .send(PlayerCommand::LoadTracklist {
                    tracks,
                    // Appending: wherever playback is, it stays there.
                    start_index: None,
                })
                .map_err(|e| tonic::Status::internal(e.to_string()))?;
        }

        Ok(tonic::Response::new(AddTracksResponse {}))
    }

    async fn clear_tracklist(
        &self,
        _request: tonic::Request<ClearTracklistRequest>,
    ) -> Result<tonic::Response<ClearTracklistResponse>, tonic::Status> {
        self.cmd_tx.lock().unwrap().send(PlayerCommand::Clear).ok();
        let response = ClearTracklistResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn filter_tracklist(
        &self,
        _request: tonic::Request<FilterTracklistRequest>,
    ) -> Result<tonic::Response<FilterTracklistResponse>, tonic::Status> {
        let response = FilterTracklistResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn get_random(
        &self,
        _request: tonic::Request<GetRandomRequest>,
    ) -> Result<tonic::Response<GetRandomResponse>, tonic::Status> {
        let response = GetRandomResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn get_repeat(
        &self,
        _request: tonic::Request<GetRepeatRequest>,
    ) -> Result<tonic::Response<GetRepeatResponse>, tonic::Status> {
        let response = GetRepeatResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn get_single(
        &self,
        _request: tonic::Request<GetSingleRequest>,
    ) -> Result<tonic::Response<GetSingleResponse>, tonic::Status> {
        let response = GetSingleResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn get_next_track(
        &self,
        _request: tonic::Request<GetNextTrackRequest>,
    ) -> Result<tonic::Response<GetNextTrackResponse>, tonic::Status> {
        let response = GetNextTrackResponse {
            track: Some(Track {
                ..Default::default()
            }),
        };
        Ok(tonic::Response::new(response))
    }

    async fn get_previous_track(
        &self,
        _request: tonic::Request<GetPreviousTrackRequest>,
    ) -> Result<tonic::Response<GetPreviousTrackResponse>, tonic::Status> {
        let response = GetPreviousTrackResponse {
            track: Some(Track {
                ..Default::default()
            }),
        };
        Ok(tonic::Response::new(response))
    }

    async fn remove_track_at(
        &self,
        request: tonic::Request<RemoveTrackAtRequest>,
    ) -> Result<tonic::Response<RemoveTrackAtResponse>, tonic::Status> {
        let request = request.into_inner();
        self.cmd_tx
            .lock()
            .unwrap()
            .send(PlayerCommand::RemoveTrack(request.position as usize))
            .unwrap();
        let response = RemoveTrackAtResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn shuffle(
        &self,
        request: tonic::Request<ShuffleRequest>,
    ) -> Result<tonic::Response<ShuffleResponse>, tonic::Status> {
        let enabled = request.into_inner().enabled;
        self.cmd_tx
            .lock()
            .unwrap()
            .send(PlayerCommand::SetShuffle(enabled))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
        let response = ShuffleResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn set_repeat(
        &self,
        request: tonic::Request<SetRepeatRequest>,
    ) -> Result<tonic::Response<SetRepeatResponse>, tonic::Status> {
        let mode = request.into_inner().mode;
        self.cmd_tx
            .lock()
            .unwrap()
            .send(PlayerCommand::SetRepeat(mode))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
        let response = SetRepeatResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn get_tracklist_tracks(
        &self,
        _request: tonic::Request<GetTracklistTracksRequest>,
    ) -> Result<tonic::Response<GetTracklistTracksResponse>, tonic::Status> {
        let (previous_tracks, next_tracks) = self.state.lock().unwrap().tracks();

        let response = GetTracklistTracksResponse {
            next_tracks: next_tracks.into_iter().map(Into::into).collect(),
            previous_tracks: previous_tracks.into_iter().map(Into::into).collect(),
        };
        Ok(tonic::Response::new(response))
    }

    async fn play_next(
        &self,
        request: tonic::Request<PlayNextRequest>,
    ) -> Result<tonic::Response<PlayNextResponse>, tonic::Status> {
        let track = request
            .into_inner()
            .track
            .ok_or_else(|| tonic::Status::invalid_argument("Track is required"))?;
        let mut track: track::Model = track.into();
        self.resolve_source_track(&mut track).await?;
        if track.uri.is_empty() {
            return Err(tonic::Status::invalid_argument("Track URI is required"));
        }
        self.cmd_tx
            .lock()
            .unwrap()
            .send(PlayerCommand::PlayNext(track))
            .map_err(|_| tonic::Status::unavailable("Player command channel closed"))?;
        let response = PlayNextResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn play_track_at(
        &self,
        request: tonic::Request<PlayTrackAtRequest>,
    ) -> Result<tonic::Response<PlayTrackAtResponse>, tonic::Status> {
        let request = request.into_inner();
        self.cmd_tx
            .lock()
            .unwrap()
            .send(PlayerCommand::PlayTrackAt(request.index as usize))
            .unwrap();
        let response = PlayTrackAtResponse {};
        Ok(tonic::Response::new(response))
    }

    async fn load_tracks(
        &self,
        request: tonic::Request<LoadTracksRequest>,
    ) -> Result<tonic::Response<LoadTracksResponse>, tonic::Status> {
        let request = request.into_inner();
        let mut tracks: Vec<track::Model> = request.tracks.into_iter().map(Into::into).collect();
        self.validate_queue(&mut tracks).await?;
        let start_index = usize::try_from(request.start_index)
            .map_err(|_| tonic::Status::invalid_argument("Invalid start index"))?;
        if start_index >= tracks.len() {
            return Err(tonic::Status::invalid_argument("Invalid start index"));
        }

        // One lock for the whole replacement: concurrent requests cannot put
        // another Clear or Load between this request's commands.
        let cmd_tx = self.cmd_tx.lock().unwrap();
        cmd_tx
            .send(PlayerCommand::Stop)
            .map_err(|_| tonic::Status::unavailable("Player command channel closed"))?;
        cmd_tx
            .send(PlayerCommand::Clear)
            .map_err(|_| tonic::Status::unavailable("Player command channel closed"))?;
        cmd_tx
            .send(PlayerCommand::LoadTracklist {
                tracks,
                start_index: Some(start_index),
            })
            .map_err(|_| tonic::Status::unavailable("Player command channel closed"))?;
        let response = LoadTracksResponse {};
        Ok(tonic::Response::new(response))
    }
}

#[cfg(test)]
#[path = "tracklist/source_tests.rs"]
mod source_tests;
