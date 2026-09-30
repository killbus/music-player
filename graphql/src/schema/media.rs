//! Container navigation keeps the account and the original Emby metadata.

use super::{objects::track::Track, provider};
use async_graphql::{Context, Error, Object, SimpleObject, ID};
use music_player_provider::{ConnectedProvider, MediaEntry as ProviderEntry, Page};

#[derive(SimpleObject)]
pub struct MediaBrowser {
    pub server_id: ID,
    pub supported: bool,
}

#[derive(SimpleObject)]
pub struct MediaEntry {
    pub id: ID,
    pub title: String,
    pub item_type: Option<String>,
    pub media_type: Option<String>,
    pub is_container: bool,
    /// Decimal strings preserve indices larger than GraphQL Int / JS Number.
    pub season_number: Option<String>,
    pub episode_number: Option<String>,
    pub track: Option<Track>,
}

#[derive(SimpleObject)]
pub struct MediaPage {
    pub server_id: ID,
    pub entries: Vec<MediaEntry>,
    pub next_offset: Option<i32>,
}

impl From<ProviderEntry> for MediaEntry {
    fn from(entry: ProviderEntry) -> Self {
        Self {
            id: ID(entry.id),
            title: entry.title,
            item_type: entry.item_type,
            media_type: entry.media_type,
            is_container: entry.is_container,
            season_number: entry.season_number.map(|number| number.to_string()),
            episode_number: entry.episode_number.map(|number| number.to_string()),
            track: entry.track.map(Into::into),
        }
    }
}

// Capture one provider before awaiting HTTP. The response names that account;
// a client switching sources can discard the old response by its query key.
async fn current_for(ctx: &Context<'_>, server_id: &str) -> Result<ConnectedProvider, Error> {
    let current = provider::connected(ctx)
        .await
        .ok_or_else(|| Error::new("Connect a media server to browse its libraries"))?;
    if current.config.id != server_id {
        return Err(Error::new(
            "The browsing server changed; refresh its libraries",
        ));
    }
    if !current.provider.capabilities().media_browse {
        return Err(Error::new(
            "This server does not support media library navigation",
        ));
    }
    Ok(current)
}

#[derive(Default)]
pub struct MediaQuery;

#[Object]
impl MediaQuery {
    async fn media_browser(&self, ctx: &Context<'_>) -> Option<MediaBrowser> {
        provider::connected(ctx).await.map(|current| MediaBrowser {
            server_id: ID(current.config.id),
            supported: current.provider.capabilities().media_browse,
        })
    }

    /// Root libraries or direct children. Containers are never playable rows.
    async fn browse_media(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        parent: Option<ID>,
        offset: Option<i32>,
        limit: Option<i32>,
    ) -> Result<MediaPage, Error> {
        let offset = offset.unwrap_or(0);
        let limit = limit.unwrap_or(100);
        if offset < 0 || !(1..=500).contains(&limit) {
            return Err(Error::new(
                "Media pages need a nonnegative offset and a limit from 1 to 500",
            ));
        }
        let next = offset
            .checked_add(limit)
            .ok_or_else(|| Error::new("Media page offset is too large"))?;
        let current = current_for(ctx, &server_id).await?;
        let mut entries = current
            .provider
            .browse(
                parent.as_ref().map(|id| id.as_str()),
                Page::new(offset, limit + 1),
            )
            .await
            .map_err(provider::err)?;
        let next_offset = (entries.len() > limit as usize).then_some(next);
        entries.truncate(limit as usize);
        for entry in &mut entries {
            entry.track = entry
                .track
                .take()
                .map(|track| provider::decorate(track, &current.config));
        }
        Ok(MediaPage {
            server_id: ID(current.config.id),
            entries: entries.into_iter().map(Into::into).collect(),
            next_offset,
        })
    }

    /// Flatten a container for queueing. limit <= 0 requests every leaf; the
    /// caller receives the complete result or an error before changing a queue.
    async fn media_container_tracks(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        parent: ID,
        offset: Option<i32>,
        limit: Option<i32>,
    ) -> Result<Vec<Track>, Error> {
        let current = current_for(ctx, &server_id).await?;
        let tracks = current
            .provider
            .container_tracks(&parent, provider::page(offset, limit))
            .await
            .map_err(provider::err)?;
        Ok(provider::decorate_all(tracks, &current.config)
            .into_iter()
            .map(Into::into)
            .collect())
    }
}
