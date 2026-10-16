//! Accessibility snapshots and document-bound interaction targets (REQ-BT-025).

mod collection;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex as StdMutex};

use chromiumoxide::cdp::browser_protocol::{
    accessibility::{AxNode, AxValue},
    dom::{BackendNodeId, ResolveNodeParams},
    input::{
        DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams,
        DispatchMouseEventType, InsertTextParams, MouseButton,
    },
    page::EventFrameNavigated,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    CallFunctionOnParams, EventBindingCalled, ReleaseObjectParams, RemoteObjectId,
};
use chromiumoxide::{layout::Point, Page};
use futures::StreamExt;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::{BrowserError, BrowserSession};

pub(crate) const INPUT_RECEIPT_BINDING: &str = "__phoenixReferenceInputReceipt";
pub(crate) const INPUT_GUARD_SCRIPT: &str = include_str!("snapshot/input_guard.js");

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

#[derive(Default)]
pub(crate) struct SnapshotState {
    current: Arc<StdMutex<SnapshotStorage>>,
    navigation_task: Option<tokio::task::JoinHandle<()>>,
    #[cfg(test)]
    retirement: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct SnapshotStorage {
    generation: u64,
    refs: Option<SnapshotReferences>,
}

impl SnapshotStorage {
    fn invalidate(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.refs = None;
        self.generation
    }
}

impl SnapshotState {
    async fn listen(&mut self, page: &Page) -> Result<(), BrowserError> {
        if self.navigation_task.is_some() {
            return Ok(());
        }
        let mut events = page.event_listener::<EventFrameNavigated>().await?;
        let current = Arc::downgrade(&self.current);
        #[cfg(test)]
        let retirement = self.retirement.clone();
        self.navigation_task = Some(tokio::spawn(async move {
            while let Some(event) = events.next().await {
                if event.frame.parent_id.is_some() {
                    continue;
                }
                let Some(current) = current.upgrade() else {
                    break;
                };
                current
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .invalidate();
                #[cfg(test)]
                retirement.notify_one();
            }
        }));
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SnapshotStorage> {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn invalidate(&self) -> u64 {
        self.lock().invalidate()
    }

    pub(crate) fn stop(&mut self) {
        if let Some(task) = self.navigation_task.take() {
            task.abort();
        }
        self.invalidate();
    }
}

impl Drop for SnapshotState {
    fn drop(&mut self) {
        self.stop();
    }
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
    let role = ax_value(node.role.as_ref());
    if matches!(role.as_str(), "RootWebArea" | "WebArea") {
        return false;
    }
    matches!(
        role.as_str(),
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
        self.snapshot.listen(&self.page).await?;
        let generation = self.snapshot.invalidate();
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
        let mut storage = self.snapshot.lock();
        if storage.generation != generation {
            return Err(error(
                "Document changed during snapshot; call browser_snapshot again",
            ));
        }
        storage.refs = Some(refs);
        Ok(output)
    }

    pub fn invalidate_snapshot(&mut self) {
        self.snapshot.invalidate();
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
        let mut storage = self.snapshot.lock();
        let refs = storage.refs.as_mut().ok_or_else(|| error(STALE))?;
        let artifact =
            SnapshotArtifact::write(runtime_env.tmp_root(), output).map_err(|failure| {
                error(format!("Could not save accessibility snapshot: {failure}"))
            })?;
        let path = artifact.path.clone();
        refs.artifact = Some(artifact);
        Ok(path)
    }

    async fn reference_object(&self, reference: &str) -> Result<ReferenceObject, BrowserError> {
        let (document, backend) = {
            let storage = self.snapshot.lock();
            let refs = storage.refs.as_ref().ok_or_else(|| error(STALE))?;
            (
                refs.document,
                *refs.nodes.get(reference).ok_or_else(|| error(STALE))?,
            )
        };
        if self.page.get_document().await?.backend_node_id != document {
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
                    .backend_node_id(backend)
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
        if !self.reference_guard_available {
            return Err(error(
                "Reference input guard unavailable; restart the browser before using element refs",
            ));
        }
        let object = self.reference_object(reference).await?;
        let mut guard = click_object(&self.page, &object.id, &self.reference_input_lock).await?;
        guard.disarm().await;
        Ok(())
    }

    /// # Errors
    /// Returns an error for stale or unfocused targets, unsupported clearing, or failed CDP input.
    pub async fn type_reference(
        &self,
        reference: &str,
        text: &str,
        clear: bool,
    ) -> Result<(), BrowserError> {
        if !self.reference_guard_available {
            return Err(error(
                "Reference input guard unavailable; restart the browser before using element refs",
            ));
        }
        let object = self.reference_object(reference).await?;
        let mut input_guard =
            click_object(&self.page, &object.id, &self.reference_input_lock).await?;
        async {
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
            input_guard.finish_typing().await
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

async fn click_object(
    page: &Page,
    object: &RemoteObjectId,
    input_lock: &std::sync::Arc<tokio::sync::Mutex<()>>,
) -> Result<ReferenceInputGuard, BrowserError> {
    let lease = input_lock.clone().lock_owned().await;
    let mut input_guard = ReferenceInputGuard::new(page, lease);
    let mut receipts = page.event_listener::<EventBindingCalled>().await?;
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
    input_guard.active = true;
    call(
        page,
        object,
        &format!(
            "function() {{ globalThis.__phoenixReferenceInputGuard.arm(this, {}); return true; }}",
            quoted(&input_guard.token)
        ),
    )
    .await?;
    for event in [
        DispatchMouseEventType::MousePressed,
        DispatchMouseEventType::MouseReleased,
    ] {
        let mut params = DispatchMouseEventParams::new(event, x, y);
        params.button = Some(MouseButton::Left);
        params.click_count = Some(1);
        page.execute(params).await?;
    }
    while let Some(receipt) = receipts.next().await {
        if receipt.name != INPUT_RECEIPT_BINDING {
            continue;
        }
        let payload: Value =
            serde_json::from_str(&receipt.payload).map_err(|failure| error(failure.to_string()))?;
        if payload["token"] != input_guard.token {
            continue;
        }
        match payload["outcome"].as_str() {
            Some("clicked") => return Ok(input_guard),
            Some("rejected") => return Err(error("Reference input rejected at dispatch because the target changed; earlier input events may have occurred. Inspect the page and call browser_snapshot again")),
            _ => return Err(error("Unrecognized reference input receipt")),
        }
    }
    Err(error("Reference input receipt unavailable; the action may have occurred. Inspect the page before retrying"))
}

struct ReferenceInputGuard {
    page: Page,
    token: String,
    active: bool,
    lease: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl ReferenceInputGuard {
    fn new(page: &Page, lease: tokio::sync::OwnedMutexGuard<()>) -> Self {
        Self {
            page: page.clone(),
            token: uuid::Uuid::new_v4().to_string(),
            active: false,
            lease: Some(lease),
        }
    }

    async fn disarm(&mut self) {
        if let Err(failure) = self
            .page
            .evaluate(format!(
                "globalThis.__phoenixReferenceInputGuard?.cancel({})",
                quoted(&self.token)
            ))
            .await
        {
            tracing::debug!(%failure, "reference input context unavailable during cleanup");
        }
        self.active = false;
        self.lease.take();
    }

    async fn finish_typing(&mut self) -> Result<(), BrowserError> {
        let result = self
            .page
            .evaluate(format!(
                "globalThis.__phoenixReferenceInputGuard.finish({})",
                quoted(&self.token)
            ))
            .await?
            .into_value::<Value>()
            .map_err(|failure| error(failure.to_string()))?;
        self.active = false;
        self.lease.take();
        if result["rejected"] == true {
            return Err(error("Reference input rejected at dispatch because the target changed; text may have been partially entered. Inspect the page and call browser_snapshot again"));
        }
        Ok(())
    }
}

impl Drop for ReferenceInputGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let page = self.page.clone();
        let token = quoted(&self.token);
        let lease = self.lease.take();
        tokio::spawn(async move {
            if let Err(failure) = page
                .evaluate(format!(
                    "globalThis.__phoenixReferenceInputGuard?.cancel({token})"
                ))
                .await
            {
                tracing::debug!(%failure, "could not disarm reference input guard");
            }
            drop(lease);
        });
    }
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

    #[tokio::test]
    async fn native_reference_guard_rejects_replacement_at_the_input_boundary() {
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let manager = crate::BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(
            WorkScopeId::parse(format!("snapshot-event-guard-{}", uuid::Uuid::new_v4())).unwrap(),
        );
        let session = manager.get_session(&scope).await.expect("Chrome session");
        {
            let mut session = session.write().await;
            assert!(session.reference_guard_available);
            session
                .page
                .goto("data:text/html,<html><body></body></html>")
                .await
                .unwrap();
            for replace_after_mousedown in [false, true] {
                session.page.evaluate(format!(r#"(() => {{
                    document.body.innerHTML = '<button id="original" style="position:fixed;left:20px;top:20px;width:100px;height:40px">Original</button>';
                    window.wrongClick = 0; window.originalDown = 0;
                    const original = document.getElementById('original');
                    function replace() {{
                        original.outerHTML = '<button style="position:fixed;left:20px;top:20px;width:100px;height:40px" onclick="window.wrongClick++">Replacement</button>';
                    }}
                    original.onmousedown = () => {{ window.originalDown++; if ({replace_after_mousedown}) replace(); }};
                    globalThis.__phoenixReferenceInputGuard.arm(original, 'test-token');
                    if (!{replace_after_mousedown}) replace();
                }})()"#)).await.unwrap();
                for event in [
                    DispatchMouseEventType::MousePressed,
                    DispatchMouseEventType::MouseReleased,
                ] {
                    let mut params = DispatchMouseEventParams::new(event, 50.0, 40.0);
                    params.button = Some(MouseButton::Left);
                    params.click_count = Some(1);
                    session.page.execute(params).await.unwrap();
                }
                let result = session.page.evaluate("JSON.stringify({outcome:globalThis.__phoenixReferenceInputGuard.finish('test-token'), wrong:window.wrongClick, down:window.originalDown})").await.unwrap().into_value::<String>().unwrap();
                let result: Value = serde_json::from_str(&result).unwrap();
                assert_eq!(result["wrong"], 0, "{result}");
                assert_eq!(
                    result["down"],
                    usize::from(replace_after_mousedown),
                    "{result}"
                );
                assert_eq!(result["outcome"]["rejected"], true, "{result}");
            }
            session
                .page
                .evaluate("document.body.innerHTML = '<button>Current</button>'")
                .await
                .unwrap();
            let snapshot = session.accessibility_snapshot(false).await.unwrap();
            assert!(snapshot.contains("Current"));
        }
        manager.shutdown_all().await.expect("browser shutdown");
    }

    #[tokio::test]
    async fn native_reference_guard_preserves_navigation_and_cancellation() {
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let manager = crate::BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(
            WorkScopeId::parse(format!("snapshot-navigation-{}", uuid::Uuid::new_v4())).unwrap(),
        );
        let session = manager.get_session(&scope).await.expect("Chrome session");
        {
            let mut session = session.write().await;
            for html in [
                "<a href='about:blank%23native-link'>Navigate link</a>",
                "<button onclick=\"location.href='about:blank?native-handler'\">Navigate handler</button>",
            ] {
                session.page.goto(format!("data:text/html,{html}")).await.unwrap();
                let snapshot = session.accessibility_snapshot(false).await.unwrap();
                let reference = snapshot.lines().find(|line| line.contains("Navigate")).unwrap().split('[').nth(1).unwrap().split(']').next().unwrap().to_string();
                let mut loaded = session
                    .page
                    .event_listener::<chromiumoxide::cdp::browser_protocol::page::EventLoadEventFired>()
                    .await
                    .unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(5), session.click_reference(&reference)).await.expect("native navigation completes").expect("native navigation receipt survives document replacement");
                tokio::time::timeout(std::time::Duration::from_secs(5), loaded.next())
                    .await
                    .expect("clicked document finishes loading")
                    .expect("load event stream remains available");
            }
            session.page.goto("about:blank").await.unwrap();
            session.page.evaluate("document.body.innerHTML = '<button onclick=window.clicks++>Current</button>'; window.clicks=0").await.unwrap();
            let snapshot = session.accessibility_snapshot(false).await.unwrap();
            let reference = snapshot
                .lines()
                .find(|line| line.contains("Current"))
                .unwrap()
                .split('[')
                .nth(1)
                .unwrap()
                .split(']')
                .next()
                .unwrap()
                .to_string();
            let object = session.reference_object(&reference).await.unwrap();
            let lease = session.reference_input_lock.clone().lock_owned().await;
            let mut guard = ReferenceInputGuard::new(&session.page, lease);
            guard.active = true;
            call(&session.page, &object.id, &format!("function() {{ globalThis.__phoenixReferenceInputGuard.arm(this, {}); return true; }}", quoted(&guard.token))).await.unwrap();
            drop(guard);
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                session.click_reference(&reference),
            )
            .await
            .expect("next operation waits for cancellation cleanup")
            .unwrap();
            assert_eq!(
                session
                    .page
                    .evaluate("window.clicks")
                    .await
                    .unwrap()
                    .into_value::<u64>()
                    .unwrap(),
                1
            );

            session.page.evaluate("document.body.innerHTML='<input id=original><input id=other>'; const original=document.getElementById('original'); original.focus(); globalThis.__phoenixReferenceInputGuard.arm(original,'type-token'); document.getElementById('other').focus()").await.unwrap();
            session
                .page
                .execute(InsertTextParams::new("must-not-retarget"))
                .await
                .unwrap();
            let result = session.page.evaluate("JSON.stringify({value:document.getElementById('other').value,outcome:globalThis.__phoenixReferenceInputGuard.finish('type-token')})").await.unwrap().into_value::<String>().unwrap();
            let result: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(result["value"], "");
            assert_eq!(result["outcome"]["rejected"], true);
        }
        manager.shutdown_all().await.expect("browser shutdown");
    }

    #[tokio::test]
    async fn native_reference_guard_rejects_pointer_capture_retargeting() {
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let manager = crate::BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(
            WorkScopeId::parse(format!("snapshot-pointer-capture-{}", uuid::Uuid::new_v4()))
                .unwrap(),
        );
        let session = manager.get_session(&scope).await.expect("Chrome session");
        let mut outcomes = Vec::new();
        {
            let mut session = session.write().await;
            session.page.goto("about:blank").await.unwrap();
            for mode in ["light", "open", "closed", "declarative-closed"] {
                if mode == "declarative-closed" {
                    session.page.goto("data:text/html,<div><template shadowrootmode=closed><button onclick='window.wrong++'>Capture target</button></template></div><script>window.wrong=0</script>").await.unwrap();
                } else {
                    session.page.evaluate(format!(r#"(() => {{
                    document.body.innerHTML = '<div id="host"></div>';
                    const mode = {mode};
                    const root = mode === 'light' ? document.body : document.getElementById('host').attachShadow({{mode}});
                    root.innerHTML = '<button id="original" style="position:fixed;left:20px;top:20px;width:100px;height:40px">Capture target</button><button id="other" style="position:fixed;left:200px;top:20px;width:100px;height:40px">Other target</button>';
                    const original = root.querySelector('#original'), other = root.querySelector('#other');
                    window.wrong = 0;
                    for (const type of ['pointerup', 'mouseup', 'click']) other.addEventListener(type, () => window.wrong++);
                    original.onpointerdown = event => other.setPointerCapture(event.pointerId);
                }})()"#, mode=quoted(mode))).await.unwrap();
                }
                let snapshot = session.accessibility_snapshot(false).await.unwrap();
                let reference = snapshot
                    .lines()
                    .find(|line| line.contains("Capture target"))
                    .unwrap()
                    .split('[')
                    .nth(1)
                    .unwrap()
                    .split(']')
                    .next()
                    .unwrap();
                let outcome = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    session.click_reference(reference),
                )
                .await;
                let wrong = session
                    .page
                    .evaluate("window.wrong")
                    .await
                    .unwrap()
                    .into_value::<u64>()
                    .unwrap();
                outcomes.push((mode, outcome, wrong));
            }
        }
        manager.shutdown_all().await.unwrap();
        for (mode, outcome, wrong) in outcomes {
            assert!(
                outcome.expect("guard rejection receipt").is_err(),
                "{mode}: retargeted click succeeded"
            );
            assert_eq!(
                wrong, 0,
                "{mode}: retargeted native event reached other node"
            );
        }
    }

    #[tokio::test]
    async fn native_reference_guard_refuses_document_reset_without_hooks() {
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let manager = crate::BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(
            WorkScopeId::parse(format!("snapshot-reset-{}", uuid::Uuid::new_v4())).unwrap(),
        );
        let session = manager.get_session(&scope).await.expect("Chrome session");
        let (result, clicked) = {
            let mut session = session.write().await;
            session.page.goto("about:blank").await.unwrap();
            session.page.evaluate("document.open(); document.write('<button onclick=window.clicked++>Reset target</button>'); document.close(); window.clicked=0").await.unwrap();
            let snapshot = session.accessibility_snapshot(false).await.unwrap();
            let reference = snapshot
                .lines()
                .find(|line| line.contains("Reset target"))
                .unwrap()
                .split('[')
                .nth(1)
                .unwrap()
                .split(']')
                .next()
                .unwrap();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                session.click_reference(reference),
            )
            .await;
            let clicked = session
                .page
                .evaluate("window.clicked")
                .await
                .unwrap()
                .into_value::<u64>()
                .unwrap();
            (result, clicked)
        };
        manager.shutdown_all().await.unwrap();
        let failure = result
            .expect("guard fails before dispatch without waiting for a receipt")
            .unwrap_err();
        assert!(
            failure.to_string().contains("guard unavailable"),
            "{failure}"
        );
        assert_eq!(
            clicked, 0,
            "no native action may escape missing guard hooks"
        );
    }

    #[tokio::test]
    async fn snapshot_artifacts_retire_on_full_navigation_without_new_snapshot() {
        use chromiumoxide::cdp::browser_protocol::page::EventNavigatedWithinDocument;
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let runtime = phoenix_core::runtime_env::PhoenixRuntimeEnvironment::with_root(root.path());
        let manager = crate::BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(
            WorkScopeId::parse(format!("snapshot-retirement-{}", uuid::Uuid::new_v4())).unwrap(),
        );
        let session = manager.get_session(&scope).await.expect("Chrome session");
        {
            let mut session = session.write().await;
            for via_eval in [false, true] {
                let html = format!("<button>{}</button>", "Accessible value ".repeat(400));
                let listening = session.snapshot.navigation_task.is_some();
                let retirement = session.snapshot.retirement.clone();
                session
                    .page
                    .goto(format!("about:blank?source={via_eval}"))
                    .await
                    .unwrap();
                session
                    .page
                    .evaluate(format!("document.body.innerHTML={}", quoted(&html)))
                    .await
                    .unwrap();
                if listening {
                    tokio::time::timeout(std::time::Duration::from_secs(5), retirement.notified())
                        .await
                        .unwrap();
                }
                let output = session.accessibility_snapshot(false).await.unwrap();
                assert!(output.len() > 4096);
                let path = session.save_snapshot_artifact(&runtime, &output).unwrap();
                let mut fragments = session
                    .page
                    .event_listener::<EventNavigatedWithinDocument>()
                    .await
                    .unwrap();
                session
                    .page
                    .evaluate("location.hash='same-document'")
                    .await
                    .unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(5), fragments.next())
                    .await
                    .expect("same-document hash navigation event")
                    .unwrap();
                assert!(
                    path.exists(),
                    "fragment navigation retains current snapshot"
                );
                if via_eval {
                    session
                        .page
                        .evaluate("location.href='about:blank?eval-replacement'")
                        .await
                        .unwrap();
                } else {
                    session
                        .page
                        .goto("about:blank?navigate-replacement")
                        .await
                        .unwrap();
                }
                tokio::time::timeout(std::time::Duration::from_secs(5), retirement.notified())
                    .await
                    .expect("main-frame retirement event");
                assert!(
                    !path.exists(),
                    "full navigation retires artifact while session write lock is held"
                );
            }
        }
        manager.shutdown_all().await.unwrap();
    }

    #[test]
    fn snapshot_does_not_offer_document_nodes_as_interaction_targets() {
        let mut root = node(1, "RootWebArea", "Page", &[2]);
        root.properties = Some(
            serde_json::from_value(json!([
                {"name":"focusable", "value":{"type":"boolean", "value":true}}
            ]))
            .unwrap(),
        );
        let (output, refs) = render(&[root, node(2, "button", "Target", &[])], false, document());
        assert!(!output
            .lines()
            .find(|line| line.contains("RootWebArea"))
            .unwrap()
            .contains('['));
        assert_eq!(refs.nodes.len(), 1);
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
