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

/// The user guide (`docs/guide/`), embedded so the in-app `/help` page can render
/// it without a checkout. Read-only markdown + the `SUMMARY.md` manifest.
#[derive(Embed)]
#[folder = "../../docs/guide"]
struct DocsGuide;

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

/// Serve an embedded user-guide doc (`docs/guide/<path>`) for the in-app help
/// page. Read-only: `DocsGuide::get` only resolves files baked in at build time,
/// so unknown paths 404 and there is no path-traversal surface; the `..` guard
/// covers the dev filesystem fallback below.
pub async fn serve_help_file(req: Request<Body>) -> impl IntoResponse {
    let rel = req.uri().path().trim_start_matches("/api/help/");
    // Reject empty, parent-traversal, and absolute paths. An absolute `rel`
    // (e.g. `/api/help//etc/passwd`, or a drive-qualified `C:/...` on Windows)
    // would make `PathBuf::join` below discard the `docs/guide` base and read an
    // arbitrary server file. `Path::is_absolute` is platform-aware.
    if rel.is_empty() || rel.contains("..") || std::path::Path::new(rel).is_absolute() {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Not found"))
            .unwrap();
    }

    // Embedded (production / release build)
    if let Some(content) = DocsGuide::get(rel) {
        let mime = mime_guess::from_path(rel).first_or_octet_stream();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(content.data.to_vec()))
            .unwrap();
    }

    // Filesystem fallback (development, server run from the repo root)
    let fs_path = PathBuf::from("docs/guide").join(rel);
    if fs_path.exists() {
        if let Ok(content) = std::fs::read(&fs_path) {
            let mime = mime_guess::from_path(rel).first_or_octet_stream();
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
            .header(header::CACHE_CONTROL, "no-cache")
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
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from(content))
                .unwrap();
        }
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Service worker not found"))
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
