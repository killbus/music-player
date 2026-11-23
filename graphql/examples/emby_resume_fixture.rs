#![recursion_limit = "512"]
//! Isolated local playback instance backed by the real schema/player/resolver.
//! Run scripts/fixtures/emby-resume.mjs first. All state is under the supplied
//! directory; no discovery, scanners, external accounts or production services.
use async_graphql::{Data, Executor, Request, Response, Schema};
use futures_util::{future::poll_fn, stream::BoxStream};
use migration::{Migrator, MigratorTrait};
use music_player_entity::track::Model as TrackModel;
use music_player_graphql::{
    schema::{
        objects::{player_state::PlayerState, track::Track},
        playback::PositionMilliseconds,
        Mutation, Query, Subscription,
    },
    simple_broker::SimpleBroker,
};
use music_player_playback::{
    player::{Player, PlayerCommand, PlayerEvent},
    source_resolver::SourceResolver,
};
use music_player_provider::{ProviderRegistry, ProviderState};
use music_player_renderer::CurrentReceiverDevice;
use music_player_storage::{saved_servers, searcher::Searcher, Database};
use music_player_tracklist::Tracklist;
use music_player_types::{
    source::{RemoteIdentity, ResourceKind, SourceRef},
    types::Device,
};
use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

// Tide polls requests on async-std. Enter the fixture's Tokio runtime for each
// poll so the unchanged production resolvers/subscriptions can use Tokio timers.
#[derive(Clone)]
struct FixtureExecutor {
    schema: Schema<Query, Mutation, Subscription>,
    runtime: tokio::runtime::Handle,
}

impl Executor for FixtureExecutor {
    async fn execute(&self, request: Request) -> Response {
        // Account login is outside this fixture. The UI's account badge must
        // not initialize the user's shared atradio token store or contact a PDS.
        if request.query.contains("query GetAccount") {
            return Response::new(async_graphql::value!({ "account": null }));
        }
        let mut response = std::pin::pin!(self.schema.execute(request));
        poll_fn(|cx| {
            let _entered = self.runtime.enter();
            response.as_mut().poll(cx)
        }).await
    }

    fn execute_stream(&self, request: Request, data: Option<Arc<Data>>) -> BoxStream<'static, Response> {
        let runtime = self.runtime.clone();
        let mut stream = self.schema.execute_stream_with_session_data(request, data.unwrap_or_default());
        Box::pin(futures_util::stream::poll_fn(move |cx| {
            let _entered = runtime.enter();
            stream.as_mut().poll_next(cx)
        }))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("supply an isolated fixture directory"),
    );
    std::fs::create_dir_all(&directory)?;
    // Keep ordinary Windows paths: settings joins subpaths using '/', which
    // Windows verbatim (\\?\) paths do not accept.
    let directory = if directory.is_absolute() {
        directory
    } else {
        std::env::current_dir()?.join(directory)
    };
    std::env::set_var("MUSIC_PLAYER_APPLICATION_DIRECTORY", &directory);
    std::env::set_var("MUSIC_PLAYER_AUDIO_OUTPUT", "tcp-connect:127.0.0.1:18097");
    std::env::set_var("MUSIC_PLAYER_LOCAL_SEARCH_ONLY", "1");
    std::env::set_var("MUSIC_PLAYER_HOST", "127.0.0.1");
    std::env::set_var("MUSIC_PLAYER_HTTP_PORT", "18098");

    let connection = sea_orm::Database::connect("sqlite::memory:").await?;
    Migrator::up(&connection, None).await?;
    let db = Database { connection };
    let account = saved_servers::upsert(
        db.get_connection(),
        &saved_servers::NewServer::new("emby", "Local resume fixture", "http://127.0.0.1:18096")
            .with_credentials(Some("fixture".into()), None)
            .with_password_update(saved_servers::PasswordUpdate::Set(String::new())),
        "2026-11-23T00:00:00Z",
    )
    .await?;
    let identity = RemoteIdentity {
        server_id: "fixture-server".into(),
        user_id: "fixture-user".into(),
    };
    saved_servers::bind_remote_identity(db.get_connection(), &account, &identity).await?;
    let tracks = ["a", "b"].map(|id| {
        let source = SourceRef {
            resolver: "emby".into(),
            account_id: account.id.clone(),
            remote: identity.clone(),
            kind: ResourceKind::Item,
            item_id: id.into(),
        }
        .to_handle();
        TrackModel {
            id: source.clone(),
            uri: source,
            title: format!("Resume fixture {id}"),
            duration: Some(if id == "b" { 6.0 } else { 60.0 }),
            ..Default::default()
        }
    });
    let queue = Arc::new(Mutex::new(Tracklist::new_empty()));
    let (sender, receiver) = mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let receiver = Arc::new(Mutex::new(receiver));
    let resolver = SourceResolver::new(db.clone(), "resume-fixture".into(), true);
    let (player, mut events) = Player::with_source_resolver(
        |event| match event {
            PlayerEvent::CurrentTrack {
                track: Some(track),
                position,
                position_ms,
                is_playing,
            } => {
                SimpleBroker::publish(Track::from(track));
                SimpleBroker::publish(PlayerState {
                    index: position as u32,
                    position_ms,
                    is_playing,
                });
            }
            PlayerEvent::TrackTimePosition { position_ms } => {
                SimpleBroker::publish(PositionMilliseconds { position_ms });
            }
            _ => {}
        },
        sender.clone(),
        receiver,
        queue.clone(),
        resolver.clone(),
    );
    tokio::spawn(async move { while events.recv().await.is_some() {} });
    // Load through the player command loop, then pause. The fixture starts
    // with a real managed occurrence ready for a UI Play click.
    sender.lock().unwrap().send(PlayerCommand::LoadTracklist {
        tracks: tracks.to_vec(),
        start_index: Some(0),
    })?;
    sender.lock().unwrap().send(PlayerCommand::Pause)?;
    let mut registry = ProviderRegistry::new();
    music_player_provider::register_builtin(&mut registry);
    let providers = Arc::new(ProviderState::new(Arc::new(registry)));
    let schema = Schema::build(
        Query::default(),
        Mutation::default(),
        Subscription::default(),
    )
    .data(resolver)
    .data(db.clone())
    .data(sender)
    .data(queue)
    .data(player.runtime())
    .data(providers)
    .data(Arc::new(AsyncMutex::new(CurrentReceiverDevice::new())))
    .data(Arc::new(Mutex::new(Vec::<Device>::new())))
    .data(Arc::new(Searcher::new(db.get_connection().clone())))
    .finish();
    let executor = FixtureExecutor { schema, runtime: tokio::runtime::Handle::current() };
    let mut app = tide::new();
    app.with(
        tide::security::CorsMiddleware::new()
            .allow_origin(tide::security::Origin::from("http://127.0.0.1:18099"))
            .allow_methods("GET, POST, OPTIONS".parse::<tide::http::headers::HeaderValue>().unwrap())
            .allow_headers("content-type".parse::<tide::http::headers::HeaderValue>().unwrap()),
    );
    app.at("/graphql")
        .get(async_graphql_tide::GraphQLSubscription::new(executor.clone()).build())
        .post(async_graphql_tide::graphql(executor));
    println!("Local resume fixture API: http://127.0.0.1:18098/graphql");
    println!("State directory: {}", directory.display());
    app.listen("127.0.0.1:18098").await?;
    Ok(())
}
