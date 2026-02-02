//! Embedded static assets for production builds
//!
//! In development, falls back to serving from filesystem.

use axum::{
    body::Body,
    http::{header, Request, Response, StatusCode},
    response::IntoResponse,
};
use rust_embed::Embed;
use std::path::PathBuf;

#[derive(Embed)]
#[folder = "ui/dist"]
struct Assets;

/// Serve embedded static files, with filesystem fallback for development
pub async fn serve_static(req: Request<Body>) -> impl IntoResponse {
    let path = req.uri().path().trim_start_matches('/');
    
    // Try embedded assets first
    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(content.data.to_vec()))
            .unwrap();
    }
    
    // Fallback to filesystem in development
    let fs_path = PathBuf::from("ui/dist").join(path);
    if fs_path.exists() {
        if let Ok(content) = std::fs::read(&fs_path) {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(content))
                .unwrap();
        }
    }
    
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not found"))
        .unwrap()
}

/// Get the index.html content (embedded or from filesystem)
pub fn get_index_html() -> Option<String> {
    // Try embedded first
    if let Some(content) = Assets::get("index.html") {
        return String::from_utf8(content.data.to_vec()).ok();
    }
    
    // Fallback to filesystem
    std::fs::read_to_string("ui/dist/index.html").ok()
}
