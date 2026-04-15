mod config;
mod db;
mod error;
mod ids;
mod pipeline;
mod routes;
mod usage;
mod worker;

use axum::Router;
use sqlx::SqlitePool;
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::config::AppConfig;
use crate::pipeline::storage::StorageClient;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: AppConfig,
    pub storage: StorageClient,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tts_podcast_backend=debug".parse().unwrap()),
        )
        .init();

    let config = AppConfig::from_env();
    tracing::info!(
        "Starting TTS Podcast backend on {}:{}",
        config.host,
        config.port
    );

    let pool = db::create_pool(&config.database_url).await;
    db::run_migrations(&pool).await;
    tracing::info!("Database connected and migrations applied");

    let storage = StorageClient::new(&config).await;

    let state = AppState {
        pool: pool.clone(),
        config: config.clone(),
        storage: storage.clone(),
    };

    // Start background workers. Each worker processes one job at a time;
    // multiple workers run concurrently and claim jobs atomically.
    worker::run_startup_recovery(&pool).await;
    for i in 0..config.worker_count {
        tokio::spawn(worker::run_worker(
            i,
            pool.clone(),
            config.clone(),
            storage.clone(),
        ));
    }
    tracing::info!("Started {} background workers", config.worker_count);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Serve SvelteKit build output as static files
    // SvelteKit adapter-static outputs to frontend/build/
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "/app/static".into());
    let index_file = format!("{}/index.html", static_dir);

    // Fallback serves index.html for SPA client-side routing
    let serve_static =
        ServeDir::new(&static_dir).not_found_service(ServeFile::new(&index_file));

    let app = Router::new()
        .merge(routes::rss_router())
        .merge(routes::api_router())
        .with_state(state)
        .fallback_service(serve_static)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .expect("Invalid address");

    tracing::info!("Listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
