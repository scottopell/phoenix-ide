//! Accessibility snapshots and document-bound interaction targets (REQ-BT-025).

mod collection;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use chromiumoxide::cdp::browser_protocol::{
    accessibility::{AxNode, AxValue},
    dom::{BackendNodeId, ResolveNodeParams},
    input::{
        DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams,
        DispatchMouseEventType, InsertTextParams, MouseButton,
    },
};
use chromiumoxide::cdp::js_protocol::runtime::{
    CallFunctionOnParams, ReleaseObjectParams, RemoteObjectId,
};
use chromiumoxide::{layout::Point, Page};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::{BrowserError, BrowserSession};

const MAX_NODES: usize = 2_000;
const MAX_BYTES: usize = 256 * 1024;
const TRUNCATED: &str =
    "[TRUNCATED: snapshot node/text limit reached; omitted nodes have no refs.]\n";
const STALE: &str = "Unknown or stale element ref; call browser_snapshot again and use a new ref";

#[derive(Debug, Clone)]
pub enum BrowserTarget {
    Selector(String),
    Reference(String),
}

impl<'de> Deserialize<'de> for BrowserTarget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut input = HashMap::<String, Value>::deserialize(deserializer)?;
        match (input.remove("selector"), input.remove("ref")) {
            (Some(Value::String(selector)), None) => Ok(Self::Selector(selector)),
            (None, Some(Value::String(reference))) => Ok(Self::Reference(reference)),
            _ => Err(serde::de::Error::custom(
                "provide exactly one of selector or ref",
            )),
        }
    }
}

#[derive(Debug)]
pub(crate) struct SnapshotReferences {
    document: BackendNodeId,
    nodes: HashMap<String, BackendNodeId>,
    artifact: Option<SnapshotArtifact>,
}

#[derive(Debug)]
struct SnapshotArtifact {
    path: std::path::PathBuf,
}

impl SnapshotArtifact {
    #[cfg(unix)]
    fn write(root: &std::path::Path, output: &str) -> std::io::Result<Self> {
        use std::io::Write as _;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        std::fs::create_dir_all(root)?;
        let directory = root.join(format!("browser-snapshot-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let artifact = Self {
            path: directory.join("snapshot.txt"),
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&artifact.path)?;
        file.write_all(output.as_bytes())?;
        Ok(artifact)
    }

    #[cfg(not(unix))]
    fn write(_root: &std::path::Path, _output: &str) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "private snapshot artifacts require owner-only filesystem permissions",
        ))
    }
}

impl Drop for SnapshotArtifact {
    fn drop(&mut self) {
        if let Err(failure) = std::fs::remove_file(&self.path) {
            if failure.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%failure, path = %self.path.display(), "could not retire snapshot artifact");
            }
        }
        if let Some(directory) = self.path.parent() {
            if let Err(failure) = std::fs::remove_dir(directory) {
                if failure.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%failure, path = %directory.display(), "could not retire snapshot artifact directory");
                }
            }
        }
    }
}

fn error(message: impl Into<String>) -> BrowserError {
    BrowserError::OperationFailed(message.into())
}

fn ax_value(value: Option<&AxValue>) -> String {
    match value.and_then(|value| value.value.as_ref()) {
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn interactive(node: &AxNode) -> bool {
    matches!(
        ax_value(node.role.as_ref()).as_str(),
        "button"
            | "link"
            | "textbox"
            | "searchbox"
            | "checkbox"
            | "radio"
            | "combobox"
            | "listbox"
            | "option"
            | "slider"
            | "spinbutton"
            | "switch"
            | "tab"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "treeitem"
    ) || node.properties.as_ref().is_some_and(|properties| {
        properties.iter().any(|property| {
            property.name.as_ref() == "focusable" && property.value.value == Some(Value::Bool(true))
        })
    })
}

fn quoted(value: &str) -> String {
    // JSON escaping keeps page-controlled newlines from masquerading as tree entries.
    serde_json::to_string(value).expect("strings serialize")
}

fn render(
    nodes: &[AxNode],
    interactive_only: bool,
    document: BackendNodeId,
) -> (String, SnapshotReferences) {
    let generation = uuid::Uuid::new_v4().simple().to_string();
    let mut refs = SnapshotReferences {
        document,
        nodes: HashMap::new(),
        artifact: None,
    };
    let index: HashMap<_, _> = nodes
        .iter()
        .map(|node| (node.node_id.clone(), node))
        .collect();
    let mut pending = nodes
        .first()
        .map(|node| vec![(node.node_id.clone(), 0)])
        .unwrap_or_default();
    let mut visited = HashSet::new();
    let mut output = String::from("Accessibility snapshot (main document; iframe contents excluded). Refs belong to this snapshot.\n");
    let mut count = 0;
    while let Some((id, depth)) = pending.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(node) = index.get(&id) else {
            continue;
        };
        let role = ax_value(node.role.as_ref());
        let show =
            !node.ignored && role != "InlineTextBox" && (!interactive_only || interactive(node));
        if show {
            let name = ax_value(node.name.as_ref());
            let mut line = format!("{}- {} {}", "  ".repeat(depth), role, quoted(&name));
            let reference = if interactive(node) {
                node.backend_dom_node_id.map(|backend| {
                    let reference = format!("@{generation}:{}", refs.nodes.len() + 1);
                    let _ = write!(line, " [{reference}]");
                    (reference, backend)
                })
            } else {
                None
            };
            if let Some(value) = node.value.as_ref() {
                let _ = write!(line, " value={}", quoted(&ax_value(Some(value))));
            }
            for property in node.properties.iter().flatten() {
                if matches!(
                    property.name.as_ref(),
                    "checked"
                        | "disabled"
                        | "expanded"
                        | "selected"
                        | "pressed"
                        | "required"
                        | "readonly"
                        | "level"
                        | "focused"
                        | "invalid"
                ) {
                    let _ = write!(
                        line,
                        " {}={}",
                        property.name.as_ref(),
                        quoted(&ax_value(Some(&property.value)))
                    );
                }
            }
            line.push('\n');
            if count == MAX_NODES || output.len() + line.len() + TRUNCATED.len() > MAX_BYTES {
                output.push_str(TRUNCATED);
                break;
            }
            output.push_str(&line);
            count += 1;
            if let Some((reference, backend)) = reference {
                refs.nodes.insert(reference, backend);
            }
        }
        // Child documents can appear in some CDP builds; do not mint main-frame refs for them.
        if role != "Iframe" && role != "IframePresentational" {
            for child in node.child_ids.iter().flatten().rev() {
                pending.push((child.clone(), depth + usize::from(show)));
            }
        }
    }
    if count == 0 && !output.ends_with(TRUNCATED) {
        output.push_str("(no matching accessible nodes)\n");
    }
    (output, refs)
}

impl BrowserSession {
    /// # Errors
    /// Returns a CDP error or a document-change error if collection is interrupted.
    pub async fn accessibility_snapshot(
        &mut self,
        interactive_only: bool,
    ) -> Result<String, BrowserError> {
        self.invalidate_snapshot();
        let document = self.page.get_document().await?.backend_node_id;
        let frame = self
            .page
            .mainframe()
            .await?
            .ok_or_else(|| error("Main frame unavailable; call browser_snapshot again"))?;
        tracing::debug!("accessibility snapshot excludes iframe documents");
        let tree = collection::collect(
            self.snapshot_websocket_address(),
            self.page.target_id(),
            frame,
        )
        .await?;
        if self.page.get_document().await?.backend_node_id != document {
            return Err(error(
                "Document changed during snapshot; call browser_snapshot again",
            ));
        }
        let (output, refs) = render(&tree.nodes, interactive_only, document);
        self.snapshot = Some(refs);
        Ok(output)
    }

    pub fn invalidate_snapshot(&mut self) {
        self.snapshot = None;
    }

    /// Save bounded snapshot text in an owner-only session-owned artifact.
    ///
    /// # Errors
    /// Returns an error when no snapshot exists or the private artifact cannot be written.
    pub fn save_snapshot_artifact(
        &mut self,
        runtime_env: &phoenix_core::runtime_env::PhoenixRuntimeEnvironment,
        output: &str,
    ) -> Result<std::path::PathBuf, BrowserError> {
        let refs = self.snapshot.as_mut().ok_or_else(|| error(STALE))?;
        let artifact =
            SnapshotArtifact::write(runtime_env.tmp_root(), output).map_err(|failure| {
                error(format!("Could not save accessibility snapshot: {failure}"))
            })?;
        let path = artifact.path.clone();
        refs.artifact = Some(artifact);
        Ok(path)
    }

    async fn reference_object(&self, reference: &str) -> Result<ReferenceObject, BrowserError> {
        let refs = self.snapshot.as_ref().ok_or_else(|| error(STALE))?;
        let backend = refs.nodes.get(reference).ok_or_else(|| error(STALE))?;
        if self.page.get_document().await?.backend_node_id != refs.document {
            return Err(error(STALE));
        }
        let context = self
            .page
            .execution_context()
            .await?
            .ok_or_else(|| error(STALE))?;
        let result = self
            .page
            .execute(
                ResolveNodeParams::builder()
                    .backend_node_id(*backend)
                    .execution_context_id(context)
                    .build(),
            )
            .await
            .map_err(|_| error(STALE))?;
        Ok(ReferenceObject {
            page: self.page.clone(),
            id: result.result.object.object_id.ok_or_else(|| error(STALE))?,
        })
    }

    /// # Errors
    /// Returns an error for stale, detached, obscured or moving targets, or failed CDP input.
    pub async fn click_reference(&self, reference: &str) -> Result<(), BrowserError> {
        let object = self.reference_object(reference).await?;
        click_object(&self.page, &object.id).await
    }

    /// # Errors
    /// Returns an error for stale or unfocused targets, unsupported clearing, or failed CDP input.
    pub async fn type_reference(
        &self,
        reference: &str,
        text: &str,
        clear: bool,
    ) -> Result<(), BrowserError> {
        let object = self.reference_object(reference).await?;
        async {
            click_object(&self.page, &object.id).await?;
            call(&self.page, &object.id, include_str!("snapshot/focused.js")).await?;
            if clear {
                call(&self.page, &object.id, "function() { if (!this.isConnected) throw Error('stale ref'); if (typeof this.select !== 'function') throw Error('clear requires a text input or textarea'); this.select(); return true; }").await?;
                call(&self.page, &object.id, include_str!("snapshot/focused.js")).await?;
                press(&self.page, "Backspace").await?;
            }
            for character in text.chars() {
                call(&self.page, &object.id, include_str!("snapshot/focused.js")).await?;
                let key = if character == '\n' { "Enter".to_string() } else { character.to_string() };
                press(&self.page, &key).await?;
            }
            Ok(())
        }.await
    }
}

struct ReferenceObject {
    page: Page,
    id: RemoteObjectId,
}

impl Drop for ReferenceObject {
    fn drop(&mut self) {
        let page = self.page.clone();
        let object = self.id.clone();
        tokio::spawn(async move {
            if let Err(error) = page.execute(ReleaseObjectParams::new(object)).await {
                tracing::debug!(%error, "could not release snapshot interaction object");
            }
        });
    }
}

async fn call(page: &Page, object: &RemoteObjectId, function: &str) -> Result<Value, BrowserError> {
    let response = page
        .execute(
            CallFunctionOnParams::builder()
                .object_id(object.clone())
                .function_declaration(function)
                .return_by_value(true)
                .build()
                .map_err(error)?,
        )
        .await
        .map_err(|e| error(format!("{STALE}: {e}")))?;
    if let Some(exception) = response.result.exception_details {
        return Err(error(format!(
            "Reference interaction failed; call browser_snapshot again: {exception:?}"
        )));
    }
    Ok(response.result.result.value.unwrap_or(Value::Null))
}

async fn click_object(page: &Page, object: &RemoteObjectId) -> Result<(), BrowserError> {
    call(page, object, "function() { if (!this.isConnected || this.ownerDocument !== document) throw Error('stale ref'); this.scrollIntoView({block:'center',inline:'center',behavior:'instant'}); return true; }").await?;
    let point = call(page, object, include_str!("snapshot/click.js")).await?;
    let x = point["x"]
        .as_f64()
        .ok_or_else(|| error("Missing reference click coordinates"))?;
    let y = point["y"]
        .as_f64()
        .ok_or_else(|| error("Missing reference click coordinates"))?;
    page.move_mouse(Point::new(x, y)).await?;
    let hovered = call(page, object, include_str!("snapshot/click.js")).await?;
    let stable = hovered["x"]
        .as_f64()
        .is_some_and(|next| (next - x).abs() < 0.5)
        && hovered["y"]
            .as_f64()
            .is_some_and(|next| (next - y).abs() < 0.5);
    if !stable {
        return Err(error(
            "Referenced element moved during hover; call browser_snapshot again",
        ));
    }
    for event in [
        DispatchMouseEventType::MousePressed,
        DispatchMouseEventType::MouseReleased,
    ] {
        let mut params = DispatchMouseEventParams::new(event, x, y);
        params.button = Some(MouseButton::Left);
        params.click_count = Some(1);
        page.execute(params).await?;
    }
    Ok(())
}

async fn press(page: &Page, key: &str) -> Result<(), BrowserError> {
    let Some(definition) = chromiumoxide::keys::get_key_definition(key) else {
        page.execute(InsertTextParams::new(key)).await?;
        return Ok(());
    };
    let text = definition
        .text
        .or_else(|| (key.chars().count() == 1).then_some(key));
    let mut down = DispatchKeyEventParams::new(if text.is_some() {
        DispatchKeyEventType::KeyDown
    } else {
        DispatchKeyEventType::RawKeyDown
    });
    down.key = Some(definition.key.to_string());
    down.code = Some(definition.code.to_string());
    down.windows_virtual_key_code = Some(definition.key_code);
    down.text = text.map(str::to_string);
    page.execute(down.clone()).await?;
    down.r#type = DispatchKeyEventType::KeyUp;
    down.text = None;
    page.execute(down).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: usize, role: &str, name: &str, children: &[usize]) -> AxNode {
        serde_json::from_value(json!({
            "nodeId": id.to_string(),
            "ignored": false,
            "role": {"type": "role", "value": role},
            "name": {"type": "computedString", "value": name},
            "backendDOMNodeId": id,
            "childIds": children.iter().map(usize::to_string).collect::<Vec<_>>()
        }))
        .expect("AX node fixture")
    }

    fn document() -> BackendNodeId {
        serde_json::from_value(json!(1)).expect("document backend ID")
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_artifacts_are_private_and_retired_with_references() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("artifact root");
        let artifact = SnapshotArtifact::write(root.path(), "private accessible value")
            .expect("private artifact");
        let path = artifact.path.clone();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let (_, mut references) = render(&[node(1, "button", "Secret", &[])], false, document());
        references.artifact = Some(artifact);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "private accessible value"
        );
        references.artifact = None;
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
        let artifact = SnapshotArtifact::write(root.path(), "replacement").unwrap();
        let path = artifact.path.clone();
        references.artifact = Some(artifact);
        drop(references);
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn snapshot_render_preserves_interactive_hierarchy_and_promotes_ignored_ancestors() {
        // REQ-BT-025: filtering removes ancestors without flattening included descendants.
        let mut ignored = node(3, "none", "Ignored wrapper", &[4]);
        ignored.ignored = true;
        let nodes = vec![
            node(1, "RootWebArea", "Page", &[2]),
            node(2, "listbox", "Choices", &[3]),
            ignored,
            node(4, "option", "First", &[]),
        ];
        let (filtered, refs) = render(&nodes, true, document());
        assert!(!filtered.contains("Page") && !filtered.contains("Ignored wrapper"));
        assert!(
            filtered
                .lines()
                .any(|line| line.starts_with("- listbox \"Choices\"")),
            "{filtered}"
        );
        assert!(
            filtered
                .lines()
                .any(|line| line.starts_with("  - option \"First\"")),
            "{filtered}"
        );
        assert_eq!(refs.nodes.len(), 2);
        let (full, _) = render(&nodes, false, document());
        assert!(
            full.lines()
                .any(|line| line.starts_with("    - option \"First\"")),
            "{full}"
        );
    }

    #[test]
    fn snapshot_render_caps_nodes_and_publishes_only_displayed_references() {
        // REQ-BT-025: truncation is explicit; omitted controls cannot be targeted.
        let children: Vec<_> = (2..MAX_NODES + 4).collect();
        let mut nodes = vec![node(1, "RootWebArea", "Page", &children)];
        nodes.extend(
            children
                .iter()
                .map(|id| node(*id, "button", &format!("Button {id}"), &[])),
        );
        let (output, refs) = render(&nodes, true, document());
        assert!(output.contains(TRUNCATED));
        assert_eq!(
            output
                .lines()
                .filter(|line| line.starts_with("- button "))
                .count(),
            MAX_NODES
        );
        assert_eq!(refs.nodes.len(), MAX_NODES);
        assert!(output.len() <= MAX_BYTES);
        assert!(!output.contains(&format!("\"Button {}\"", MAX_NODES + 2)));
        for reference in refs.nodes.keys() {
            assert!(output.contains(&format!("[{reference}]")));
        }
    }

    #[test]
    fn snapshot_render_caps_bytes_including_truncation_notice() {
        // REQ-BT-025: the notice shares the byte budget with the tree.
        let large_name = "x".repeat(MAX_BYTES / 2);
        let nodes = vec![
            node(1, "RootWebArea", "Page", &[2, 3]),
            node(2, "button", &large_name, &[]),
            node(3, "button", &large_name, &[]),
        ];
        let (output, refs) = render(&nodes, true, document());
        assert!(output.ends_with(TRUNCATED));
        assert!(output.len() <= MAX_BYTES);
        assert_eq!(refs.nodes.len(), 1);
        assert_eq!(output.matches("[@").count(), refs.nodes.len());
        let oversized = vec![node(1, "button", &"界".repeat(MAX_BYTES), &[])];
        let (output, refs) = render(&oversized, true, document());
        assert!(output.contains(TRUNCATED));
        assert!(output.len() <= MAX_BYTES);
        assert!(refs.nodes.is_empty());
        assert!(!output.contains("no matching accessible nodes"));
    }

    #[test]
    fn snapshot_render_escapes_control_characters_and_rotates_reference_generation() {
        // REQ-BT-025: page text cannot forge entries, and a new snapshot has fresh refs.
        let mut control = node(1, "textbox", "Customer\n- button \"Forged\"\t\u{0}", &[]);
        control.value = Some(
            serde_json::from_value(json!({"type": "string", "value": "line\nnext\r\t"})).unwrap(),
        );
        let (first, refs) = render(&[control.clone()], true, document());
        let (second, fresh) = render(&[control], true, document());
        assert_eq!(first.lines().count(), 2, "{first}");
        assert!(
            first.contains("Customer\\n- button \\\"Forged\\\"\\t\\u0000"),
            "{first}"
        );
        assert!(first.contains("value=\"line\\nnext\\r\\t\""), "{first}");
        let old_ref = refs.nodes.keys().next().expect("first reference");
        let new_ref = fresh.nodes.keys().next().expect("fresh reference");
        assert_ne!(old_ref, new_ref);
        assert!(!second.contains(old_ref));
        assert_eq!(refs.nodes[old_ref], fresh.nodes[new_ref]);
    }
}
