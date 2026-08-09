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
#[folder = "../../ui/dist"]
struct Assets;

const REVALIDATE: &str = "no-cache";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

fn index_response(content: String) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, REVALIDATE)
        .body(Body::from(content))
        .unwrap()
}

fn asset_response(content: Vec<u8>, mime: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, IMMUTABLE)
        .body(Body::from(content))
        .unwrap()
}

/// Serve embedded static files, with filesystem fallback for development
pub async fn serve_static(req: Request<Body>) -> impl IntoResponse {
    let path = req.uri().path().trim_start_matches('/');

    // Try embedded assets first
    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return asset_response(content.data.to_vec(), mime.as_ref());
    }

    // Fallback to filesystem in development
    let fs_path = PathBuf::from("ui/dist").join(path);
    if fs_path.exists() {
        if let Ok(content) = std::fs::read(&fs_path) {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            return asset_response(content, mime.as_ref());
        }
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not found"))
        .unwrap()
}

/// Serve the favicon (phoenix.svg)
pub async fn serve_favicon() -> impl IntoResponse {
    // Try embedded asset first
    if let Some(content) = Assets::get("phoenix.svg") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/svg+xml")
            .header(header::CACHE_CONTROL, "public, max-age=31536000")
            .body(Body::from(content.data.to_vec()))
            .unwrap();
    }

    // Fallback to filesystem in development
    let fs_path = PathBuf::from("ui/dist/phoenix.svg");
    if fs_path.exists() {
        if let Ok(content) = std::fs::read(&fs_path) {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/svg+xml")
                .header(header::CACHE_CONTROL, "public, max-age=31536000")
                .body(Body::from(content))
                .unwrap();
        }
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Favicon not found"))
        .unwrap()
}

/// Serve the service worker file
pub async fn serve_service_worker() -> impl IntoResponse {
    // Try embedded asset first
    if let Some(content) = Assets::get("service-worker.js") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/javascript")
            .header(header::CACHE_CONTROL, REVALIDATE)
            .body(Body::from(content.data.to_vec()))
            .unwrap();
    }

    // Fallback to filesystem in development
    let fs_path = PathBuf::from("ui/dist/service-worker.js");
    if fs_path.exists() {
        if let Ok(content) = std::fs::read(&fs_path) {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/javascript")
                .header(header::CACHE_CONTROL, REVALIDATE)
                .body(Body::from(content))
                .unwrap();
        }
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Service worker not found"))
        .unwrap()
}

/// Get the index.html response (embedded or from filesystem).
pub fn get_index_response() -> Option<Response<Body>> {
    let content = if let Some(content) = Assets::get("index.html") {
        String::from_utf8(content.data.to_vec()).ok()?
    } else {
        std::fs::read_to_string("ui/dist/index.html").ok()?
    };

    Some(index_response(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spa_entry_point_revalidates() {
        let response = index_response("<!doctype html>".to_string());

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], REVALIDATE);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn content_addressed_assets_are_immutable() {
        let response = asset_response(b"body {}".to_vec(), "text/css");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], IMMUTABLE);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/css");
    }
}
