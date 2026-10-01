//! Resolve queued sources by their saved account, independent of library browsing.
//! Each operation authenticates a fresh snapshot; no client or credentials are cached.

use music_player_provider::{
    backends::emby::Emby,
    emby_playback::{AudioSelection, ResolvedAudio},
    MusicProvider, ProviderConfig, ProviderError,
};
use music_player_storage::{saved_servers, saved_servers::SavedServer, Database};
use music_player_types::{
    audio::AudioOptions,
    source::{ResourceKind, SourceRef},
    types::Track,
};
use std::{sync::Arc, time::Duration};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct SourceResolver {
    db: Database,
    device_id: String,
    follow_redirects: bool,
}

impl SourceResolver {
    /// The host supplies its device identity and redirect setting (default true).
    pub fn new(db: Database, device_id: String, follow_redirects: bool) -> Self {
        Self {
            db,
            device_id,
            follow_redirects,
        }
    }

    /// The deadline covers storage, authentication, identity verification and
    /// playback resolution together. Dropping the future cancels this work; an
    /// acquired encoding lease retains the provider's bounded Drop cleanup.
    pub async fn resolve(
        &self,
        source: &SourceRef,
        selection: &AudioSelection,
        offset_ms: u64,
    ) -> Result<ResolvedAudio, ProviderError> {
        tokio::time::timeout(RESOLVE_TIMEOUT, async {
            self.authenticated(source)
                .await?
                .resolve_audio(source, selection, offset_ms)
                .await
        })
        .await
        .map_err(|_| failure("saved source resolution timed out"))?
    }

    /// Read selectable metadata through the source's saved account. Browsing
    /// another server never changes this lookup, and no playback lease is opened.
    pub async fn audio_options(&self, source: &SourceRef) -> Result<AudioOptions, ProviderError> {
        tokio::time::timeout(RESOLVE_TIMEOUT, async {
            self.authenticated(source).await?.audio_options(source).await
        })
        .await
        .map_err(|_| failure("saved source audio options lookup timed out"))?
    }

    /// Resolve an id-only queue entry through its stable handle, never a bare
    /// item ID interpreted against whichever provider is currently being browsed.
    pub async fn track(&self, source: &SourceRef) -> Result<Track, ProviderError> {
        tokio::time::timeout(RESOLVE_TIMEOUT, async {
            self.authenticated(source)
                .await?
                .track(&source.to_handle())
                .await
        })
        .await
        .map_err(|_| failure("saved source metadata lookup timed out"))?
    }

    /// Validate queue ownership without authentication or any network request.
    /// Playback still checks a fresh snapshot when it actually opens the source.
    pub async fn validate_saved(&self, source: &SourceRef) -> Result<(), ProviderError> {
        tokio::time::timeout(RESOLVE_TIMEOUT, self.saved_snapshot(source))
            .await
            .map_err(|_| failure("saved source account lookup timed out"))??;
        Ok(())
    }

    async fn saved_snapshot(&self, source: &SourceRef) -> Result<SavedServer, ProviderError> {
        // SourceRef has public fields: validate constructed values as well as
        // handles already parsed by an API. Reject before making any request.
        SourceRef::parse(&source.to_handle())
            .map_err(|_| failure("invalid saved source reference"))?;
        if source.kind != ResourceKind::Item {
            return Err(failure("saved source is not an item"));
        }
        let snapshot = saved_servers::get(self.db.get_connection(), &source.account_id)
            .await
            .map_err(|_| failure("saved source account lookup failed"))?
            .ok_or_else(|| failure("saved source account is missing"))?;
        if snapshot.kind != "emby"
            || snapshot.remote_server_id.as_deref() != Some(source.remote.server_id.as_str())
            || snapshot.remote_user_id.as_deref() != Some(source.remote.user_id.as_str())
        {
            return Err(failure(
                "saved source account identity is not bound or does not match",
            ));
        }
        Ok(snapshot)
    }

    async fn authenticated(&self, source: &SourceRef) -> Result<Arc<Emby>, ProviderError> {
        let snapshot = self.saved_snapshot(source).await?;
        let config = ProviderConfig {
            id: snapshot.id.clone(),
            kind: snapshot.kind.clone(),
            name: snapshot.name.clone(),
            url: snapshot.url.clone(),
            username: snapshot.username.clone(),
            password: snapshot.password.clone(),
        };
        let client = Emby::authenticate(&config, &self.device_id, self.follow_redirects).await?;
        if client.identity() != &source.remote {
            return Err(failure(
                "authenticated source identity does not match the saved source",
            ));
        }
        // Use the PRE-authentication snapshot, not a refreshed row. This checks
        // URL/credentials/deletion atomically before requesting item metadata or
        // a playback session. Storage deliberately permits name-only changes.
        // This playback path never establishes an initially absent binding.
        saved_servers::bind_remote_identity(self.db.get_connection(), &snapshot, client.identity())
            .await
            .map_err(|_| failure("saved source account changed during authentication"))?;
        Ok(Arc::new(client))
    }
}

fn failure(message: &'static str) -> ProviderError {
    ProviderError::Other(message.into())
}

#[cfg(test)]
mod tests;
