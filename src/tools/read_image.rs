//! Read image tool - allows LLM to examine images in the workspace
//!
//! REQ-TOOL: `read_image` for viewing screenshots, diagrams, etc.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Maximum image size (5MB)
const MAX_IMAGE_SIZE: u64 = 5 * 1024 * 1024;

/// Supported image formats and their media types
const SUPPORTED_FORMATS: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

/// Read image tool for examining images in the workspace
///
/// REQ-BASH-010: Stateless - uses `ToolContext` for `working_dir`
pub struct ReadImageTool;

impl ReadImageTool {
    fn resolve_path(ctx: &ToolContext, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            ctx.working_dir.join(path)
        }
    }

    fn get_media_type(path: &Path) -> Option<&'static str> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        SUPPORTED_FORMATS
            .iter()
            .find(|(e, _)| *e == ext)
            .map(|(_, media_type)| *media_type)
    }
}

#[derive(Debug, Deserialize)]
struct ReadImageInput {
    path: String,
    #[serde(default)]
    #[allow(dead_code)] // Part of schema, parsed but not used
    timeout: Option<String>,
}

#[async_trait]
impl Tool for ReadImageTool {
    fn name(&self) -> &'static str {
        "read_image"
    }

    fn description(&self) -> String {
        "View an image file (PNG, JPG, etc.). Required after browser_take_screenshot to see the screenshot content — the screenshot is saved to a temp file but not automatically visible until you call read_image on that path. Also use for any image file in the working directory.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the image file to read"
                },
                "timeout": {
                    "type": "string",
                    "description": "Timeout duration (default: 15s). Examples: '5s', '1m', '500ms'"
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: ReadImageInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let path = Self::resolve_path(&ctx, &input.path);

        // Check file exists
        if !path.exists() {
            return ToolOutput::error(format!("File not found: {}", path.display()));
        }

        // Check it's a file
        if !path.is_file() {
            return ToolOutput::error(format!("Not a file: {}", path.display()));
        }

        // Check format is supported
        let Some(media_type) = Self::get_media_type(&path) else {
            let supported: Vec<_> = SUPPORTED_FORMATS.iter().map(|(e, _)| *e).collect();
            return ToolOutput::error(format!(
                "Unsupported image format. Supported: {}",
                supported.join(", ")
            ));
        };

        // Check file size
        let metadata = match fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) => return ToolOutput::error(format!("Cannot read file: {e}")),
        };

        if metadata.len() > MAX_IMAGE_SIZE {
            return ToolOutput::error(format!(
                "Image too large: {} bytes (max {} bytes)",
                metadata.len(),
                MAX_IMAGE_SIZE
            ));
        }

        // Read and encode
        let data = match fs::read(&path).await {
            Ok(d) => d,
            Err(e) => return ToolOutput::error(format!("Failed to read file: {e}")),
        };

        let base64_data = BASE64.encode(&data);

        // Return structured output for the LLM
        let output = json!({
            "type": "image",
            "media_type": media_type,
            "data": base64_data,
        });

        ToolOutput::success(output.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browser::BrowserSessionManager;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn test_context(working_dir: PathBuf) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
        )
    }

    fn create_test_image(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content).unwrap();
        path
    }

    // Minimal valid PNG (1x1 transparent pixel)
    const MINIMAL_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[tokio::test]
    async fn test_read_png() {
        let dir = TempDir::new().unwrap();
        let tool = ReadImageTool;
        let ctx = test_context(dir.path().to_path_buf());
        create_test_image(dir.path(), "test.png", MINIMAL_PNG);

        let result = tool.run(json!({"path": "test.png"}), ctx).await;
        assert!(result.success, "Failed: {}", result.output);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["type"], "image");
        assert_eq!(output["media_type"], "image/png");
        assert!(!output["data"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_read_absolute_path() {
        let dir = TempDir::new().unwrap();
        let tool = ReadImageTool;
        let ctx = test_context(PathBuf::from("/tmp"));
        let img_path = create_test_image(dir.path(), "abs.png", MINIMAL_PNG);

        let result = tool
            .run(json!({"path": img_path.to_str().unwrap()}), ctx)
            .await;
        assert!(result.success, "Failed: {}", result.output);
    }

    #[tokio::test]
    async fn test_file_not_found() {
        let dir = TempDir::new().unwrap();
        let tool = ReadImageTool;
        let ctx = test_context(dir.path().to_path_buf());

        let result = tool.run(json!({"path": "nonexistent.png"}), ctx).await;
        assert!(!result.success);
        assert!(result.output.contains("not found"));
    }

    #[tokio::test]
    async fn test_unsupported_format() {
        let dir = TempDir::new().unwrap();
        let tool = ReadImageTool;
        let ctx = test_context(dir.path().to_path_buf());
        create_test_image(dir.path(), "test.bmp", b"fake bmp data");

        let result = tool.run(json!({"path": "test.bmp"}), ctx).await;
        assert!(!result.success);
        assert!(result.output.contains("Unsupported"));
    }

    #[tokio::test]
    async fn test_directory_rejected() {
        let dir = TempDir::new().unwrap();
        let tool = ReadImageTool;
        let ctx = test_context(dir.path().to_path_buf());
        std::fs::create_dir(dir.path().join("subdir")).unwrap();

        let result = tool.run(json!({"path": "subdir"}), ctx).await;
        assert!(!result.success);
        assert!(result.output.contains("Not a file"));
    }

    #[tokio::test]
    async fn test_all_supported_formats() {
        let dir = TempDir::new().unwrap();
        let tool = ReadImageTool;

        for (ext, expected_type) in SUPPORTED_FORMATS {
            let filename = format!("test.{}", ext);
            create_test_image(dir.path(), &filename, b"fake image data");

            let ctx = test_context(dir.path().to_path_buf());
            let result = tool.run(json!({"path": filename}), ctx).await;
            assert!(result.success, "Failed for {}: {}", ext, result.output);

            let output: Value = serde_json::from_str(&result.output).unwrap();
            assert_eq!(output["media_type"], *expected_type);
        }
    }
}
