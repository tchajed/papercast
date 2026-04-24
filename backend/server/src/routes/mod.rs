pub mod admin;
pub mod episodes;
pub mod feeds;
pub mod rss;

use crate::AppState;
use axum::Router;

pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(feeds::router())
        .merge(episodes::router())
        .merge(admin::router())
}

pub fn rss_router() -> Router<AppState> {
    rss::router()
}
