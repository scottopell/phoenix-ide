use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum TextCategory {
    Markdown,
    Code,
    Config,
    Plain,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FileViewerKind {
    Text { category: TextCategory },
    Image,
    Opaque,
}

impl FileViewerKind {
    pub fn for_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);
        let text = |category| Self::Text { category };

        match ext.as_deref() {
            Some("md" | "markdown") => text(TextCategory::Markdown),
            Some(
                "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "cpp" | "c" | "h"
                | "hpp" | "css" | "html" | "htm" | "vue" | "svelte" | "php" | "rb" | "swift" | "kt"
                | "scala" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "sql" | "graphql" | "proto",
            ) => text(TextCategory::Code),
            Some(
                "json" | "yaml" | "yml" | "toml" | "ini" | "env" | "conf" | "cfg" | "xml"
                | "properties",
            ) => text(TextCategory::Config),
            Some("txt" | "log" | "csv" | "tsv" | "rtf") => text(TextCategory::Plain),
            Some(
                "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "tiff" | "tif",
            ) => Self::Image,
            Some(
                "db" | "sqlite" | "sqlite3" | "bin" | "dat" | "exe" | "dll" | "so" | "dylib" | "o"
                | "a" | "wasm" | "class" | "jar" | "war" | "pyc" | "pyo" | "pdf" | "doc" | "docx"
                | "xls" | "xlsx" | "ppt" | "pptx" | "zip" | "tar" | "gz" | "bz2" | "xz" | "7z"
                | "rar" | "mp3" | "mp4" | "wav" | "avi" | "mkv" | "mov" | "webm" | "flac" | "ogg"
                | "woff" | "woff2" | "ttf" | "otf",
            ) => Self::Opaque,
            _ => text(TextCategory::Unknown),
        }
    }
}
