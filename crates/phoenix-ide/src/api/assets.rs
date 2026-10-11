//! Embedded static assets for production builds
//!
//! In development, falls back to serving from filesystem.

use axum::{
    body::Body,
    http::{header, Request, Response, StatusCode, Uri},
    response::IntoResponse,
};
#[cfg(not(debug_assertions))]
use rust_embed::Embed;
use std::path::{Component, Path};

#[cfg(not(debug_assertions))]
#[derive(Embed)]
#[folder = "../../ui/dist"]
struct Assets;

#[derive(Clone, Copy)]
pub struct PublicRootAsset {
    pub path: &'static str,
    pub content_type: &'static str,
}

pub const PUBLIC_ROOT_ASSETS: &[PublicRootAsset] = &[
    PublicRootAsset {
        path: "/manifest.webmanifest",
        content_type: "application/manifest+json",
    },
    PublicRootAsset {
        path: "/manifest-light.webmanifest",
        content_type: "application/manifest+json",
    },
    PublicRootAsset {
        path: "/apple-touch-icon.png",
        content_type: "image/png",
    },
    PublicRootAsset {
        path: "/icon-192.png",
        content_type: "image/png",
    },
    PublicRootAsset {
        path: "/icon-512.png",
        content_type: "image/png",
    },
];

pub fn public_root_asset(path: &str) -> Option<PublicRootAsset> {
    PUBLIC_ROOT_ASSETS
        .iter()
        .copied()
        .find(|asset| asset.path == path)
}

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

fn valid_asset_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn read_ui_asset(root: &Path, path: &str) -> Option<Vec<u8>> {
    if !valid_asset_path(path) {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let file = root.join(path).canonicalize().ok()?;
    if !file.starts_with(&root) {
        return None;
    }
    std::fs::read(file).ok()
}

fn embedded_asset(path: &str) -> Option<Vec<u8>> {
    #[cfg(debug_assertions)]
    {
        // rust-embed's debug lookup permits symlinks outside its source folder.
        read_ui_asset(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/dist")),
            path,
        )
    }
    #[cfg(not(debug_assertions))]
    {
        Assets::get(path).map(|content| content.data.to_vec())
    }
}

/// Serve embedded static files, with filesystem fallback for development
pub async fn serve_static(req: Request<Body>) -> impl IntoResponse {
    serve_static_from(&req, Path::new("ui/dist"))
}

fn serve_static_from(req: &Request<Body>, root: &Path) -> Response<Body> {
    let path = req.uri().path().strip_prefix('/').unwrap_or_default();
    if !valid_asset_path(path) {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Try embedded assets first
    if let Some(content) = embedded_asset(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return asset_response(content, mime.as_ref());
    }

    // Fallback to filesystem in development
    if let Some(content) = read_ui_asset(root, path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return asset_response(content, mime.as_ref());
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not found"))
        .unwrap()
}

/// Serve a root-level web application asset.
pub async fn serve_root_asset(uri: Uri) -> impl IntoResponse {
    let Some(asset) = public_root_asset(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = asset.path.trim_start_matches('/');

    if let Some(content) = embedded_asset(path) {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, asset.content_type)
            .header(header::CACHE_CONTROL, "public, max-age=86400")
            .body(Body::from(content))
            .unwrap();
    }

    if let Some(content) = read_ui_asset(Path::new("ui/dist"), path) {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, asset.content_type)
            .header(header::CACHE_CONTROL, "public, max-age=86400")
            .body(Body::from(content))
            .unwrap();
    }

    StatusCode::NOT_FOUND.into_response()
}

/// Serve the favicon (phoenix.svg)
pub async fn serve_favicon() -> impl IntoResponse {
    // Try embedded asset first
    if let Some(content) = embedded_asset("phoenix.svg") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/svg+xml")
            .header(header::CACHE_CONTROL, "public, max-age=31536000")
            .body(Body::from(content))
            .unwrap();
    }

    // Fallback to filesystem in development
    if let Some(content) = read_ui_asset(Path::new("ui/dist"), "phoenix.svg") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/svg+xml")
            .header(header::CACHE_CONTROL, "public, max-age=31536000")
            .body(Body::from(content))
            .unwrap();
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Favicon not found"))
        .unwrap()
}

/// Serve the service worker file
pub async fn serve_service_worker() -> impl IntoResponse {
    // Try embedded asset first
    if let Some(content) = embedded_asset("service-worker.js") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/javascript")
            .header(header::CACHE_CONTROL, REVALIDATE)
            .body(Body::from(content))
            .unwrap();
    }

    // Fallback to filesystem in development
    if let Some(content) = read_ui_asset(Path::new("ui/dist"), "service-worker.js") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/javascript")
            .header(header::CACHE_CONTROL, REVALIDATE)
            .body(Body::from(content))
            .unwrap();
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Service worker not found"))
        .unwrap()
}

/// Get the index.html response (embedded or from filesystem).
pub fn get_index_response() -> Option<Response<Body>> {
    let content = if let Some(content) = embedded_asset("index.html") {
        String::from_utf8(content).ok()?
    } else {
        String::from_utf8(read_ui_asset(Path::new("ui/dist"), "index.html")?).ok()?
    };

    Some(index_response(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_assets_reject_non_relative_components() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ordinary.css"), "body {}").unwrap();
        for path in [
            "",
            ".",
            "..",
            "./ordinary.css",
            "assets/../ordinary.css",
            "assets/./ordinary.css",
            "/ordinary.css",
            "//ordinary.css",
            "assets//ordinary.css",
            "assets/",
            "assets\\ordinary.css",
            "C:\\ordinary.css",
        ] {
            assert!(read_ui_asset(dir.path(), path).is_none(), "{path:?}");
        }
        let absolute = dir.path().join("ordinary.css");
        assert!(read_ui_asset(dir.path(), absolute.to_str().unwrap()).is_none());
    }

    #[tokio::test]
    async fn raw_static_paths_cannot_traverse_outside_ui_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dist");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(dir.path().join("outside-sentinel.css"), "PRIVATE SENTINEL").unwrap();
        // A direct filesystem join would expose this sibling file.
        assert_eq!(
            std::fs::read(root.join("assets/../../outside-sentinel.css")).unwrap(),
            b"PRIVATE SENTINEL"
        );
        for path in [
            "/../outside-sentinel.css",
            "/assets/../../outside-sentinel.css",
            "/assets/./../../outside-sentinel.css",
            "//../outside-sentinel.css",
            "/assets/%2e%2e/%2e%2e/outside-sentinel.css",
        ] {
            let req = Request::builder().uri(path).body(Body::empty()).unwrap();
            assert_eq!(req.uri().path(), path);
            let response = serve_static_from(&req, &root);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert!(!body
                .windows(b"PRIVATE SENTINEL".len())
                .any(|w| w == b"PRIVATE SENTINEL"));
        }
    }

    #[tokio::test]
    async fn ordinary_filesystem_asset_keeps_content_type_and_cache_policy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        let path = "assets/containment-test-only.css";
        std::fs::write(dir.path().join(path), "body { color: red; }").unwrap();
        let req = Request::builder()
            .uri(format!("/{path}?v=1"))
            .body(Body::empty())
            .unwrap();
        let response = serve_static_from(&req, dir.path());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/css");
        assert_eq!(response.headers()[header::CACHE_CONTROL], IMMUTABLE);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap(),
            "body { color: red; }"
        );
    }

    #[tokio::test]
    async fn embedded_assets_keep_precedence_over_filesystem_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "not the embedded index").unwrap();
        let expected = embedded_asset("index.html").expect("UI build supplies index.html");
        let req = Request::builder()
            .uri("/index.html")
            .body(Body::empty())
            .unwrap();
        let response = serve_static_from(&req, dir.path());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
        assert_eq!(response.headers()[header::CACHE_CONTROL], IMMUTABLE);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
            expected
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_symlinks_cannot_escape_ui_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dist");
        // A string-prefix containment check would mistake this sibling for the root.
        let outside = dir.path().join("dist-private");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel.css"), "PRIVATE SENTINEL").unwrap();
        symlink(&outside, root.join("escaped-assets")).unwrap();
        symlink(outside.join("sentinel.css"), root.join("escaped.css")).unwrap();
        for path in ["escaped.css", "escaped-assets/sentinel.css"] {
            assert!(read_ui_asset(&root, path).is_none());
            let req = Request::builder()
                .uri(format!("/{path}"))
                .body(Body::empty())
                .unwrap();
            let response = serve_static_from(&req, &root);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 1024)
                    .await
                    .unwrap(),
                "Not found"
            );
        }

        std::fs::write(root.join("ordinary.css"), "body {}").unwrap();
        symlink(root.join("ordinary.css"), root.join("internal.css")).unwrap();
        assert_eq!(read_ui_asset(&root, "internal.css").unwrap(), b"body {}");
        let root_alias = dir.path().join("approved-root-alias");
        symlink(&root, &root_alias).unwrap();
        assert_eq!(
            read_ui_asset(&root_alias, "ordinary.css").unwrap(),
            b"body {}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fixed_asset_fallbacks_also_deny_outside_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dist");
        std::fs::create_dir(&root).unwrap();
        let outside = dir.path().join("sentinel");
        std::fs::write(&outside, "PRIVATE SENTINEL").unwrap();
        let paths = PUBLIC_ROOT_ASSETS
            .iter()
            .map(|asset| asset.path.trim_start_matches('/'))
            .chain(["phoenix.svg", "service-worker.js", "index.html"]);
        for path in paths {
            symlink(&outside, root.join(path)).unwrap();
            assert!(read_ui_asset(&root, path).is_none(), "{path}");
            std::fs::remove_file(root.join(path)).unwrap();
            std::fs::write(root.join(path), "public content").unwrap();
            assert_eq!(read_ui_asset(&root, path).unwrap(), b"public content");
        }
    }

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
