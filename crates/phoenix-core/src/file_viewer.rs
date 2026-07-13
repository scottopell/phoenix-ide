use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerFileClass {
    Text,
    Image,
    Opaque,
}

pub fn classify_for_viewer(path: &Path) -> ViewerFileClass {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase);
    if matches!(
        extension.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "tiff" | "tif")
    ) {
        ViewerFileClass::Image
    } else if has_opaque_extension(path) {
        ViewerFileClass::Opaque
    } else {
        ViewerFileClass::Text
    }
}

pub fn has_opaque_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_lowercase)
            .as_deref(),
        Some(
            "db" | "sqlite"
                | "sqlite3"
                | "bin"
                | "dat"
                | "exe"
                | "dll"
                | "so"
                | "dylib"
                | "o"
                | "a"
                | "wasm"
                | "class"
                | "jar"
                | "war"
                | "pyc"
                | "pyo"
                | "pdf"
                | "doc"
                | "docx"
                | "xls"
                | "xlsx"
                | "ppt"
                | "pptx"
                | "zip"
                | "tar"
                | "gz"
                | "bz2"
                | "xz"
                | "7z"
                | "rar"
                | "mp3"
                | "mp4"
                | "wav"
                | "avi"
                | "mkv"
                | "mov"
                | "webm"
                | "flac"
                | "ogg"
                | "woff"
                | "woff2"
                | "ttf"
                | "otf"
        )
    )
}
