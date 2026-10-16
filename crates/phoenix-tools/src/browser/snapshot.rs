use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Deserialize)]
struct SnapshotInput {
    #[serde(default)]
    interactive: bool,
    timeout: Option<String>,
}

pub struct BrowserSnapshotTool;

#[async_trait]
impl Tool for BrowserSnapshotTool {
    fn name(&self) -> &'static str {
        "browser_snapshot"
    }

    fn description(&self) -> String {
        "Read the main document's accessibility tree: roles, accessible names, values and states. Set interactive:true for controls only. Pass an element's opaque ref to browser_click or browser_type instead of a selector. Refs expire on the next snapshot attempt, document replacement or browser replacement; take another snapshot when stale. Shadow content exposed by Chrome is included; iframe contents are excluded. Large output is saved to a file.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({"type":"object", "properties": {
            "interactive": {"type":"boolean", "description":"Only show interactive/focusable nodes (default false)."},
            "timeout": {"type":"string", "description":"Collection timeout (default 15s), e.g. 5s, 500ms."}
        }})
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        Self::run_browser(input, &ctx).await
    }
}

impl BrowserSnapshotTool {
    pub(super) async fn run_browser(
        input: Value,
        access: &(impl super::BrowserAccess + ?Sized),
    ) -> ToolOutput {
        let input: SnapshotInput = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(error) => return ToolOutput::error(format!("Invalid input: {error}")),
        };
        let timeout = match input.timeout.as_deref() {
            Some(value) => match super::tools::parse_duration(value) {
                Some(timeout) => timeout,
                None => {
                    return ToolOutput::error(
                        "Invalid timeout; use a duration such as 15s or 500ms",
                    )
                }
            },
            None => Duration::from_secs(15),
        };
        let session = match access.browser().await {
            Ok(session) => session,
            Err(error) => return ToolOutput::error(format!("Failed to get browser: {error}")),
        };
        let mut session = session.write().await;
        session.last_activity = std::time::Instant::now();
        let result =
            tokio::time::timeout(timeout, session.accessibility_snapshot(input.interactive)).await;
        session.last_activity = std::time::Instant::now();
        match result {
            Ok(Ok(output)) if output.len() <= 4096 => ToolOutput::success(output),
            Ok(Ok(output)) => {
                let notice = output
                    .lines()
                    .find(|line| line.starts_with("[TRUNCATED:"))
                    .unwrap_or("")
                    .to_string();
                let directory =
                    match PhoenixRuntimeEnvironment::detect().tmp_subdir("browser-snapshots") {
                        Ok(directory) => directory,
                        Err(error) => {
                            session.invalidate_snapshot();
                            return ToolOutput::error(format!(
                                "Could not create snapshot directory: {error}"
                            ));
                        }
                    };
                let path = directory.join(format!("phoenix-snapshot-{}.txt", uuid::Uuid::new_v4()));
                match tokio::fs::write(&path, output).await {
                    Ok(()) => ToolOutput::success(format!("Accessibility snapshot saved to {}. Read this file for the tree and element refs.\n{notice}", path.display())).with_display(json!({"artifact_path":path})),
                    Err(error) => {
                        session.invalidate_snapshot();
                        ToolOutput::error(format!("Could not save accessibility snapshot: {error}"))
                    }
                }
            }
            Ok(Err(error)) => ToolOutput::error(format!("Snapshot failed: {error}")),
            Err(_) => ToolOutput::error(format!(
                "Snapshot timed out after {timeout:?}; call browser_snapshot again"
            )),
        }
    }
}
