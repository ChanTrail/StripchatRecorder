//! 前端静态资源嵌入与 handler / Embedded frontend static assets and handler

use axum::{
    http::{StatusCode, Uri, header},
    response::IntoResponse,
};
use rust_embed::RustEmbed;

/// Embedded frontend static assets (compiled from ../build_tmp/frontend/dist/ into the binary).
#[derive(RustEmbed)]
#[folder = "../build_tmp/frontend/dist/"]
pub struct FrontendAssets;

pub async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match FrontendAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => match FrontendAssets::get("index.html") {
            Some(content) => ([(header::CONTENT_TYPE, "text/html")], content.data).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}
