use anyhow::Error;
use music_player_server::api::{
    metadata::v1alpha1::{Album, Artist, Track},
    music::v1alpha1::{
        library_service_client::LibraryServiceClient, BrowseMediaRequest, BrowseMediaResponse,
        GetAlbumDetailsRequest, GetAlbumsRequest, GetArtistDetailsRequest, GetArtistsRequest,
        GetMediaBrowserRequest, GetMediaContainerTracksRequest, GetMediaContainerTracksResponse,
        GetTrackDetailsRequest, GetTracksRequest, MediaBrowser, SearchRequest, SearchResponse,
    },
};
use tonic::transport::Channel;

pub struct LibraryClient {
    client: LibraryServiceClient<Channel>,
}

impl LibraryClient {
    /// Older daemons return Unimplemented; do not substitute their music list.
    pub async fn media_browser(&mut self) -> Result<Option<MediaBrowser>, Error> {
        Ok(self
            .client
            .get_media_browser(GetMediaBrowserRequest {})
            .await?
            .into_inner()
            .browser)
    }

    /// The response includes its original account and exact uint64 indices.
    pub async fn browse_media(
        &mut self,
        server_id: &str,
        parent: Option<&str>,
        offset: i32,
        limit: Option<i32>,
    ) -> Result<BrowseMediaResponse, Error> {
        Ok(self
            .client
            .browse_media(BrowseMediaRequest {
                server_id: server_id.into(),
                parent: parent.map(str::to_owned),
                offset,
                limit,
            })
            .await?
            .into_inner())
    }

    /// Some(0) expands all leaves without modifying the queue.
    pub async fn media_container_tracks(
        &mut self,
        server_id: &str,
        parent: &str,
        offset: i32,
        limit: Option<i32>,
    ) -> Result<GetMediaContainerTracksResponse, Error> {
        Ok(self
            .client
            .get_media_container_tracks(GetMediaContainerTracksRequest {
                server_id: server_id.into(),
                parent: parent.into(),
                offset,
                limit,
            })
            .await?
            .into_inner())
    }

    pub async fn new(host: String, port: u16) -> Result<Self, Error> {
        let url = format!("http://{}:{}", host, port);
        let client = LibraryServiceClient::connect(url).await?;
        Ok(Self { client })
    }

    pub async fn album(&mut self, id: &str) -> Result<Option<Album>, Error> {
        let request = tonic::Request::new(GetAlbumDetailsRequest { id: id.to_string() });
        let response = self.client.get_album_details(request).await?;
        Ok(response.into_inner().album)
    }

    pub async fn albums(
        &mut self,
        filter: Option<String>,
        offset: i32,
        limit: i32,
    ) -> Result<Vec<Album>, Error> {
        let filter = match filter {
            Some(filter) => filter,
            None => "".to_string(),
        };
        let request = tonic::Request::new(GetAlbumsRequest {
            offset,
            limit,
            filter,
        });
        let response = self.client.get_albums(request).await?;
        Ok(response.into_inner().albums.into_iter().collect())
    }

    pub async fn artist(&mut self, id: &str) -> Result<Option<Artist>, Error> {
        let request = tonic::Request::new(GetArtistDetailsRequest { id: id.to_string() });
        let response = self.client.get_artist_details(request).await?;
        Ok(response.into_inner().artist)
    }

    pub async fn artists(
        &mut self,
        filter: Option<String>,
        offset: i32,
        limit: i32,
    ) -> Result<Vec<Artist>, Error> {
        let filter = match filter {
            Some(filter) => filter,
            None => "".to_string(),
        };
        let request = tonic::Request::new(GetArtistsRequest {
            offset,
            limit,
            filter,
        });
        let response = self.client.get_artists(request).await?;
        Ok(response.into_inner().artists.into_iter().collect())
    }

    pub async fn songs(
        &mut self,
        filter: Option<String>,
        offset: i32,
        limit: i32,
    ) -> Result<Vec<Track>, Error> {
        let filter = match filter {
            Some(filter) => filter,
            None => "".to_string(),
        };
        let request = tonic::Request::new(GetTracksRequest {
            offset,
            limit,
            filter,
        });
        let response = self.client.get_tracks(request).await?;
        Ok(response.into_inner().tracks.into_iter().collect())
    }

    pub async fn song(&mut self, id: &str) -> Result<Option<Track>, Error> {
        let request = tonic::Request::new(GetTrackDetailsRequest { id: id.to_string() });
        let response = self.client.get_track_details(request).await?;
        Ok(response.into_inner().track)
    }

    pub async fn search(&mut self, query: &str) -> Result<SearchResponse, Error> {
        let request = tonic::Request::new(SearchRequest {
            query: query.to_string(),
        });
        let response = self.client.search(request).await?;
        Ok(response.into_inner())
    }
}
