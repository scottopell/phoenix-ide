pub use phoenix_svg as validation;
pub use phoenix_svg::{
    SvgArtifactReference, SvgInvocationId, SvgPresentationMetadata, SvgValidationOutcome,
    MAX_DESCRIPTION_CHARS, MAX_TITLE_CHARS,
};

use super::{Tool, ToolContext, ToolExecutionEnvironment, ToolOutput};
use async_trait::async_trait;
use phoenix_core::domain::sm_state::PresentSvgInput;
use phoenix_core::work_scope::{ResourceAuthority, ResourceScopeKey};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
pub enum CoordinatorSvgSourceError {
    Authority,
    Persistence,
    Read,
}

#[async_trait]
pub trait CoordinatorSvgSourceResolver: Send + Sync {
    async fn resolve_active_work_scope_root(
        &self,
        work_scope_id: &str,
    ) -> Result<PathBuf, CoordinatorSvgSourceError>;
}

pub struct CoordinatorPresentSvgTool {
    resolver: Arc<dyn CoordinatorSvgSourceResolver>,
}

impl CoordinatorPresentSvgTool {
    pub fn new(resolver: Arc<dyn CoordinatorSvgSourceResolver>) -> Self {
        Self { resolver }
    }
}

pub struct SvgArtifactDraft {
    pub metadata: SvgPresentationMetadata,
    pub svg: validation::ValidatedSvg,
}

#[async_trait]
pub trait SvgArtifactStore: Send + Sync {
    async fn lookup(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
    ) -> Result<Option<SvgArtifactReference>, String>;
    async fn publish(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
        draft: SvgArtifactDraft,
    ) -> Result<SvgArtifactReference, String>;
}

#[async_trait]
impl<T: SvgArtifactStore + ?Sized> SvgArtifactStore for Arc<T> {
    async fn lookup(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
    ) -> Result<Option<SvgArtifactReference>, String> {
        (**self).lookup(conversation_id, invocation).await
    }
    async fn publish(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
        draft: SvgArtifactDraft,
    ) -> Result<SvgArtifactReference, String> {
        (**self).publish(conversation_id, invocation, draft).await
    }
}

pub struct PresentSvgTool;

fn failure(category: &str, message: &str) -> ToolOutput {
    ToolOutput::error(format!("present_svg {category}: {message}"))
}

fn reference_output(reference: &SvgArtifactReference) -> ToolOutput {
    match serde_json::to_string(reference) {
        Ok(output) => ToolOutput::success(output),
        Err(_) => failure(
            "persistence_failure",
            "Could not encode the published reference.",
        ),
    }
}

type FileReadError = (&'static str, &'static str);

fn read_opened_regular_file(file: std::fs::File) -> Result<Vec<u8>, FileReadError> {
    let metadata = file
        .metadata()
        .map_err(|_| ("read_failure", "Cannot inspect opened source."))?;
    if !metadata.is_file() {
        return Err((
            "invalid_input",
            "Source must be a regular file, not a directory, device, or pipe.",
        ));
    }
    if metadata.len() > validation::MAX_BYTES as u64 {
        return Err(("limit", "SVG exceeds the 2 MiB byte limit."));
    }
    let mut bytes = Vec::new();
    file.take(validation::MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ("read_failure", "Could not read source bytes."))?;
    if bytes.len() > validation::MAX_BYTES {
        return Err(("limit", "SVG exceeds the 2 MiB byte limit."));
    }
    Ok(bytes)
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, FileReadError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| {
        (
            "read_failure",
            "Cannot open source. Use a readable regular file, not a symlink.",
        )
    })?;
    read_opened_regular_file(file)
}

#[cfg(unix)]
fn read_regular_file_beneath(root: &Path, path: &Path) -> Result<Vec<u8>, FileReadError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let relative = path.strip_prefix(root).map_err(|_| {
        (
            "policy_rejection",
            "Source must be contained by the selected active WorkScope root.",
        )
    })?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err((
            "policy_rejection",
            "Source must be a contained regular file without traversal components.",
        ));
    }

    let root = CString::new(root.as_os_str().as_bytes()).map_err(|_| {
        (
            "policy_rejection",
            "Selected WorkScope root is not a valid server path.",
        )
    })?;
    let root_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err((
            "read_failure",
            "Cannot open the selected active WorkScope root.",
        ));
    }
    let mut directory = unsafe { std::fs::File::from_raw_fd(root_fd) };

    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            unreachable!();
        };
        let name = CString::new(name.as_bytes()).map_err(|_| {
            (
                "policy_rejection",
                "Source path contains an invalid component.",
            )
        })?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err((
                "read_failure",
                "Cannot open source beneath the selected WorkScope root; symlinks are unsupported.",
            ));
        }
        directory = unsafe { std::fs::File::from_raw_fd(fd) };
    }

    let Component::Normal(filename) = components[components.len() - 1] else {
        unreachable!();
    };
    let filename = CString::new(filename.as_bytes()).map_err(|_| {
        (
            "policy_rejection",
            "Source filename contains an invalid component.",
        )
    })?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            filename.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err((
            "read_failure",
            "Cannot open source beneath the selected WorkScope root; symlinks are unsupported.",
        ));
    }
    read_opened_regular_file(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(not(unix))]
fn read_regular_file_beneath(_root: &Path, _path: &Path) -> Result<Vec<u8>, FileReadError> {
    Err((
        "policy_rejection",
        "Contained no-follow Coordinator publication is unsupported on this platform.",
    ))
}

enum PublicationSource {
    ConversationFilesystem,
    CoordinatorWorkScope(PathBuf),
}

impl PresentSvgTool {
    /// Publish from one server-resolved active `WorkScope` without making the
    /// Coordinator's otherwise filesystem-free tool context ambiently writable.
    async fn run_for_coordinator_work_scope(
        &self,
        input: Value,
        ctx: ToolContext,
        canonical_work_scope_root: PathBuf,
    ) -> ToolOutput {
        if !matches!(
            ctx.execution_environment,
            ToolExecutionEnvironment::NoFilesystem
        ) || ctx.work_scope != ResourceScopeKey::Coordinator
        {
            return failure(
                "policy_rejection",
                "Coordinator publication requires its filesystem-free runtime context.",
            );
        }
        self.publish(
            input,
            ctx,
            PublicationSource::CoordinatorWorkScope(canonical_work_scope_root),
        )
        .await
    }

    async fn lookup_existing(&self, ctx: &ToolContext) -> Option<ToolOutput> {
        let store = ctx.svg_artifact_store.as_ref()?;
        let assistant_message_id = ctx.svg_assistant_message_id.as_ref()?;
        let tool_use_id = ctx.tool_use_id()?;
        let invocation = SvgInvocationId::new(assistant_message_id, tool_use_id);
        match store.lookup(&ctx.conversation_id, &invocation).await {
            Ok(Some(reference)) => Some(reference_output(&reference)),
            Ok(None) => None,
            Err(_) => Some(failure(
                "persistence_failure",
                "Could not check durable publication identity; retry later.",
            )),
        }
    }

    async fn publish(
        &self,
        input: Value,
        ctx: ToolContext,
        source: PublicationSource,
    ) -> ToolOutput {
        let Some(store) = &ctx.svg_artifact_store else {
            return failure(
                "persistence_failure",
                "Durable publication is unavailable in this execution context.",
            );
        };
        let (Some(assistant_message_id), Some(tool_use_id)) =
            (&ctx.svg_assistant_message_id, ctx.tool_use_id())
        else {
            return failure(
                "persistence_failure",
                "Invocation identity is missing; retry through the conversation runtime.",
            );
        };
        let invocation = SvgInvocationId::new(assistant_message_id, tool_use_id);
        match store.lookup(&ctx.conversation_id, &invocation).await {
            Ok(Some(reference)) => return reference_output(&reference),
            Ok(None) => {}
            Err(_) => {
                return failure(
                    "persistence_failure",
                    "Could not check durable publication identity; retry later.",
                )
            }
        }
        let Ok(input) = serde_json::from_value::<PresentSvgInput>(input) else {
            return failure(
                "invalid_input",
                "Provide path, title, and description as strings.",
            );
        };
        if !Path::new(&input.path).is_absolute()
            || input.path.len() > 4096
            || input.path.contains('\0')
        {
            return failure("invalid_input", "Use a resolved absolute server filename of at most 4096 bytes; shell expressions are not expanded.");
        }
        let metadata = match SvgPresentationMetadata::new(&input.title, &input.description) {
            Ok(metadata) => metadata,
            Err(error) => return failure("invalid_input", error.message),
        };
        if ctx.cancel.is_cancelled() {
            return failure("cancelled", "Publication cancelled before reading.");
        }
        let path = PathBuf::from(input.path);
        let validated = tokio::task::spawn_blocking(move || {
            let bytes = match source {
                PublicationSource::ConversationFilesystem => read_regular_file(&path),
                PublicationSource::CoordinatorWorkScope(root) => {
                    read_regular_file_beneath(&root, &path)
                }
            }?;
            validation::validate(&bytes).map_err(|error| {
                let category = match error.category {
                    validation::ValidationCategory::InvalidInput => "invalid_input",
                    validation::ValidationCategory::Policy => "policy_rejection",
                    validation::ValidationCategory::Limit => "limit",
                };
                (category, error.message)
            })
        })
        .await;
        let svg = match validated {
            Ok(Ok(svg)) => svg,
            Ok(Err((category, message))) => return failure(category, message),
            Err(_) => return failure("read_failure", "Source validation could not complete."),
        };
        if ctx.cancel.is_cancelled() {
            return failure("cancelled", "Publication cancelled before persistence.");
        }
        match store
            .publish(
                &ctx.conversation_id,
                &invocation,
                SvgArtifactDraft { metadata, svg },
            )
            .await
        {
            Ok(reference) => reference_output(&reference),
            Err(_) => failure(
                "persistence_failure",
                "Could not commit SVG snapshot and ownership; retry later.",
            ),
        }
    }
}

#[async_trait]
impl Tool for CoordinatorPresentSvgTool {
    fn name(&self) -> &'static str {
        "present_svg"
    }

    fn description(&self) -> String {
        "Publish a static SVG staged inside one active WorkScope as a durable inline visual owned by this Global Coordinator transcript. First generate the file through Coordinator bash using that WorkScope's explicit work_scope_id, then call present_svg with the same work_scope_id and resolved absolute SERVER filename. The server re-resolves the active WorkScope and permits only a contained regular file without symlinks. Keep staging until success; the tool does not remove it. Validation enforces the existing bounded static SVG policy and is not visual inspection. Success returns a compact reference, never SVG bytes."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        let mut schema = PresentSvgTool.input_schema();
        schema["properties"]["work_scope_id"] = json!({
            "type": "string",
            "minLength": 1,
            "description": "Authoritative active WorkScope used to stage the SVG through Coordinator bash. Phoenix re-resolves its canonical root server-side."
        });
        schema["required"] = json!(["work_scope_id", "path", "title", "description"]);
        schema
    }

    async fn run(&self, mut input: Value, ctx: ToolContext) -> ToolOutput {
        let Some(object) = input.as_object_mut() else {
            return failure(
                "invalid_input",
                "Provide work_scope_id, path, title, and description as strings.",
            );
        };
        let Some(work_scope_id) = object
            .remove("work_scope_id")
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|value| !value.trim().is_empty())
        else {
            return failure(
                "invalid_input",
                "work_scope_id must name one active WorkScope.",
            );
        };

        if let Some(reference) = PresentSvgTool.lookup_existing(&ctx).await {
            return reference;
        }

        let root = match self
            .resolver
            .resolve_active_work_scope_root(&work_scope_id)
            .await
        {
            Ok(root) => root,
            Err(CoordinatorSvgSourceError::Authority) => {
                return failure(
                    "policy_rejection",
                    "Active persisted WorkScope with a live owner not found.",
                )
            }
            Err(CoordinatorSvgSourceError::Persistence) => {
                return failure(
                    "persistence_failure",
                    "Could not resolve the selected WorkScope; retry later.",
                )
            }
            Err(CoordinatorSvgSourceError::Read) => {
                return failure(
                    "read_failure",
                    "Selected WorkScope root is unavailable or invalid.",
                )
            }
        };
        PresentSvgTool
            .run_for_coordinator_work_scope(input, ctx, root)
            .await
    }
}

#[async_trait]
impl Tool for PresentSvgTool {
    fn name(&self) -> &'static str {
        "present_svg"
    }

    fn description(&self) -> String {
        "Publish a static SVG file as a durable inline visual for the user. Generate SVG with code or chart libraries, then pass its resolved absolute SERVER filename; never paste markup into arguments. Direct/Work only. Stage with artifact_dir=$(mktemp -d \"${TMPDIR:-/tmp}/phoenix-svg.XXXXXX\"); print the generated filename and call present_svg separately. Delete staging only after success (this tool leaves it intact). Max 2 MiB, title 200 characters, description 2000; both nonempty plain text. Static shapes/text, safe styling, local glyph/use/clip/gradient references only; no DOCTYPE, scripts, links, animation, embedded HTML, images or external resources. Library output must omit DOCTYPE/metadata. Max 20,000 elements, depth 64, 100,000 attributes/path segments, 10,000 references, 500,000 expanded complexity; dimensions <=16,384 px and area <=64 million px. Validation is not visual inspection. Success returns a compact reference, never SVG bytes. Revisions require another invocation.".into()
    }

    fn input_schema(&self) -> Value {
        json!({"type":"object", "additionalProperties":false,
        "required":["path","title","description"], "properties":{
            "path":{"type":"string","maxLength":4096,"description":"Resolved absolute server filename. No ~ or shell-variable expansion."},
            "title":{"type":"string","minLength":1,"maxLength":MAX_TITLE_CHARS},
            "description":{"type":"string","minLength":1,"maxLength":MAX_DESCRIPTION_CHARS,"description":"Plain-text accessible description of what the visual communicates."}
        }})
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        if !matches!(
            ctx.execution_environment,
            ToolExecutionEnvironment::Filesystem(_)
        ) || ctx.resource_access.authority() != ResourceAuthority::Work
        {
            return failure(
                "policy_rejection",
                "Publication requires Direct or Work filesystem authority; Explore is unsupported.",
            );
        }
        self.publish(input, ctx, PublicationSource::ConversationFilesystem)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    type StoredArtifacts = HashMap<(String, SvgInvocationId), (SvgArtifactReference, Vec<u8>)>;
    #[derive(Default)]
    struct Store(Mutex<StoredArtifacts>);
    #[async_trait]
    impl SvgArtifactStore for Store {
        async fn lookup(
            &self,
            conversation_id: &str,
            invocation: &SvgInvocationId,
        ) -> Result<Option<SvgArtifactReference>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&(conversation_id.into(), invocation.clone()))
                .map(|(r, _)| r.clone()))
        }
        async fn publish(
            &self,
            conversation_id: &str,
            invocation: &SvgInvocationId,
            draft: SvgArtifactDraft,
        ) -> Result<SvgArtifactReference, String> {
            let mut entries = self.0.lock().unwrap();
            let entry = entries
                .entry((conversation_id.into(), invocation.clone()))
                .or_insert_with(|| {
                    (
                        SvgArtifactReference {
                            artifact_id: uuid::Uuid::new_v4().to_string(),
                            conversation_id: conversation_id.into(),
                            title: draft.metadata.title().into(),
                            description: draft.metadata.description().into(),
                            width: draft.svg.width(),
                            height: draft.svg.height(),
                            validation: SvgValidationOutcome::AcceptedStaticSvg,
                        },
                        draft.svg.into_bytes(),
                    )
                });
            Ok(entry.0.clone())
        }
    }
    fn context(store: Arc<dyn SvgArtifactStore>, id: &str) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "owner".into(),
            std::env::current_dir().unwrap(),
            Arc::new(crate::BrowserSessionManager::default()),
            Arc::new(crate::BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(crate::TmuxRegistry::new()),
            None,
            phoenix_core::work_scope::WorkScopeId::parse("svg-test").unwrap(),
        )
        .with_tool_use_id(id)
        .with_svg_assistant_message_id("assistant-first")
        .with_svg_artifact_store(store)
    }
    fn input(path: &Path) -> Value {
        json!({"path":path,"title":"Disk usage","description":"Measured directory sizes in GiB"})
    }
    const SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='50'><rect width='80' height='20'/></svg>";

    struct Resolver(Mutex<Result<PathBuf, CoordinatorSvgSourceError>>);

    #[async_trait]
    impl CoordinatorSvgSourceResolver for Resolver {
        async fn resolve_active_work_scope_root(
            &self,
            _work_scope_id: &str,
        ) -> Result<PathBuf, CoordinatorSvgSourceError> {
            match &*self.0.lock().unwrap() {
                Ok(path) => Ok(path.clone()),
                Err(CoordinatorSvgSourceError::Authority) => {
                    Err(CoordinatorSvgSourceError::Authority)
                }
                Err(CoordinatorSvgSourceError::Persistence) => {
                    Err(CoordinatorSvgSourceError::Persistence)
                }
                Err(CoordinatorSvgSourceError::Read) => Err(CoordinatorSvgSourceError::Read),
            }
        }
    }

    fn coordinator_input(path: &Path) -> Value {
        let mut value = input(path);
        value["work_scope_id"] = json!("scope-1");
        value
    }

    fn coordinator_context(store: Arc<dyn SvgArtifactStore>, id: &str) -> ToolContext {
        ToolContext::new_without_filesystem(
            CancellationToken::new(),
            "global-transcript".into(),
            Arc::new(crate::BrowserSessionManager::default()),
            Arc::new(crate::BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(crate::TmuxRegistry::new()),
        )
        .with_tool_use_id(id)
        .with_svg_assistant_message_id("global-assistant")
        .with_svg_artifact_store(store)
    }

    #[tokio::test]
    async fn coordinator_publication_uses_global_invocation_and_contained_source() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let file = root_path.join("chart.svg");
        std::fs::write(&file, SVG).unwrap();
        let store = Arc::new(Store::default());
        let result = PresentSvgTool
            .run_for_coordinator_work_scope(
                input(&file),
                coordinator_context(store.clone(), "global-tool-use"),
                root_path,
            )
            .await;
        assert!(result.is_success(), "{}", result.output());
        let entries = store.0.lock().unwrap();
        assert!(entries.contains_key(&(
            "global-transcript".into(),
            SvgInvocationId::new("global-assistant", "global-tool-use")
        )));
    }

    #[tokio::test]
    async fn coordinator_publication_rejects_escape_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), SVG).unwrap();
        let store = Arc::new(Store::default());
        let root_path = root.path().canonicalize().unwrap();
        let escaped = PresentSvgTool
            .run_for_coordinator_work_scope(
                input(outside.path()),
                coordinator_context(store.clone(), "escape"),
                root_path.clone(),
            )
            .await;
        assert!(!escaped.is_success());
        assert!(escaped.output().contains("policy_rejection"));

        #[cfg(unix)]
        {
            let link = root_path.join("linked.svg");
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            let linked = PresentSvgTool
                .run_for_coordinator_work_scope(
                    input(&link),
                    coordinator_context(store.clone(), "link"),
                    root_path,
                )
                .await;
            assert!(!linked.is_success());
            assert!(linked.output().contains("read_failure"));
        }
        assert!(store.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn coordinator_replay_precedes_retired_scope_resolution() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let file = root_path.join("chart.svg");
        std::fs::write(&file, SVG).unwrap();
        let store = Arc::new(Store::default());
        let resolver = Arc::new(Resolver(Mutex::new(Ok(root_path))));
        let tool = CoordinatorPresentSvgTool::new(resolver.clone());
        let first = tool
            .run(
                coordinator_input(&file),
                coordinator_context(store.clone(), "replay"),
            )
            .await;
        assert!(first.is_success(), "{}", first.output());

        *resolver.0.lock().unwrap() = Err(CoordinatorSvgSourceError::Authority);
        std::fs::remove_file(&file).unwrap();
        let replay = tool
            .run(
                coordinator_input(&file),
                coordinator_context(store, "replay"),
            )
            .await;
        assert!(replay.is_success(), "{}", replay.output());
        assert_eq!(replay.output(), first.output());
    }

    #[tokio::test]
    async fn coordinator_resolver_errors_keep_distinct_categories() {
        for (error, category) in [
            (CoordinatorSvgSourceError::Authority, "policy_rejection"),
            (
                CoordinatorSvgSourceError::Persistence,
                "persistence_failure",
            ),
            (CoordinatorSvgSourceError::Read, "read_failure"),
        ] {
            let tool = CoordinatorPresentSvgTool::new(Arc::new(Resolver(Mutex::new(Err(error)))));
            let result = tool
                .run(
                    json!({
                        "work_scope_id": "scope-1",
                        "path": "/not/read.svg",
                        "title": "Title",
                        "description": "Description"
                    }),
                    coordinator_context(Arc::new(Store::default()), category),
                )
                .await;
            assert!(!result.is_success());
            assert!(result.output().contains(category), "{}", result.output());
        }
    }

    #[tokio::test]
    async fn coordinator_entry_rejects_ordinary_filesystem_context() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("chart.svg");
        std::fs::write(&file, SVG).unwrap();
        let store = Arc::new(Store::default());
        let result = PresentSvgTool
            .run_for_coordinator_work_scope(
                input(&file),
                context(store.clone(), "wrong-context"),
                root.path().canonicalize().unwrap(),
            )
            .await;
        assert!(!result.is_success());
        assert!(result.output().contains("policy_rejection"));
        assert!(store.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn replay_retains_snapshot_after_source_deleted() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), SVG).unwrap();
        let store = Arc::new(Store::default());
        let ctx = context(store.clone(), "first");
        let args = input(file.path());
        let first = PresentSvgTool.run(args.clone(), ctx.clone()).await;
        assert!(first.is_success(), "{}", first.output());
        assert!(!first.output().contains("<svg"));
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), SVG);
        file.close().unwrap();
        let replay = PresentSvgTool.run(args, ctx).await;
        assert_eq!(first.output(), replay.output());
        let entries = store.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries.values().next().unwrap().1, SVG.as_bytes());
    }

    #[tokio::test]
    async fn reused_provider_id_in_another_assistant_message_publishes_new_snapshot() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), SVG).unwrap();
        let store = Arc::new(Store::default());
        let first = PresentSvgTool
            .run(input(file.path()), context(store.clone(), "reused"))
            .await;
        let revised = SVG.replace("80", "60");
        std::fs::write(file.path(), &revised).unwrap();
        let second_context =
            context(store.clone(), "reused").with_svg_assistant_message_id("assistant-second");
        let second = PresentSvgTool
            .run(input(file.path()), second_context.clone())
            .await;
        assert!(first.is_success());
        assert!(second.is_success());
        assert_ne!(first.output(), second.output());
        let args = input(file.path());
        file.close().unwrap();
        let replay = PresentSvgTool.run(args, second_context).await;
        assert_eq!(second.output(), replay.output());
        let entries = store.0.lock().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .values()
            .any(|(_, bytes)| bytes == revised.as_bytes()));
    }

    #[tokio::test]
    async fn invalid_inputs_and_cancel_do_not_publish() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), SVG).unwrap();
        let store = Arc::new(Store::default());
        for args in [
            json!({}),
            json!({"path":"relative.svg","title":"x","description":"y"}),
            json!({"path":file.path(),"title":" ","description":"y"}),
            json!({"path":file.path(),"title":"x","description":"z".repeat(MAX_DESCRIPTION_CHARS+1)}),
        ] {
            assert!(!PresentSvgTool
                .run(args, context(store.clone(), "input"))
                .await
                .is_success());
        }
        let ctx = context(store.clone(), "cancelled");
        ctx.cancel.cancel();
        assert!(!PresentSvgTool
            .run(input(file.path()), ctx)
            .await
            .is_success());
        let ctx = context(store.clone(), "explore")
            .with_resource_authority(ResourceAuthority::Restricted);
        assert!(!PresentSvgTool
            .run(input(file.path()), ctx)
            .await
            .is_success());
        assert!(store.0.lock().unwrap().is_empty());
    }

    struct FailingStore;
    #[async_trait]
    impl SvgArtifactStore for FailingStore {
        async fn lookup(
            &self,
            _: &str,
            _: &SvgInvocationId,
        ) -> Result<Option<SvgArtifactReference>, String> {
            Ok(None)
        }
        async fn publish(
            &self,
            _: &str,
            _: &SvgInvocationId,
            _: SvgArtifactDraft,
        ) -> Result<SvgArtifactReference, String> {
            Err("simulated storage failure with private details".into())
        }
    }

    #[tokio::test]
    async fn persistence_and_policy_failures_are_bounded_without_source_leaks() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), SVG).unwrap();
        let failure = PresentSvgTool
            .run(
                input(file.path()),
                context(Arc::new(FailingStore), "failed"),
            )
            .await;
        assert!(!failure.is_success());
        assert!(failure.output().contains("persistence_failure"));
        assert!(!failure.output().contains("private details"));
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), SVG);
        std::fs::write(file.path(), "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><script>SECRET CONTENT</script></svg>").unwrap();
        let store = Arc::new(Store::default());
        let failure = PresentSvgTool
            .run(input(file.path()), context(store.clone(), "invalid"))
            .await;
        assert!(!failure.is_success());
        assert!(failure.output().contains("policy_rejection"));
        assert!(!failure.output().contains("SECRET"));
        assert!(store.0.lock().unwrap().is_empty());
    }

    #[test]
    fn metadata_limits_count_unicode_characters() {
        assert!(SvgPresentationMetadata::new(&"é".repeat(MAX_TITLE_CHARS), "Description").is_ok());
        assert!(
            SvgPresentationMetadata::new(&"é".repeat(MAX_TITLE_CHARS + 1), "Description").is_err()
        );
        assert!(SvgPresentationMetadata::new("Title", "\n").is_err());
        assert!(SvgPresentationMetadata::new("Title", "x\0y").is_err());
    }

    #[test]
    fn file_boundary_rejects_directory_device_symlink_and_oversize() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_regular_file(dir.path()).unwrap_err().0,
            "invalid_input"
        );
        let path = dir.path().join("large.svg");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(validation::MAX_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(read_regular_file(&path).unwrap_err().0, "limit");
        assert_eq!(
            read_regular_file(&dir.path().join("missing"))
                .unwrap_err()
                .0,
            "read_failure"
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert_eq!(read_regular_file(&link).unwrap_err().0, "read_failure");
            assert_eq!(
                read_regular_file(Path::new("/dev/null")).unwrap_err().0,
                "invalid_input"
            );
            let fifo = dir.path().join("fifo");
            let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            assert_eq!(read_regular_file(&fifo).unwrap_err().0, "invalid_input");
        }
    }

    #[test]
    fn mode_exposure_matches_cross_call_staging_support() {
        let has = |registry: crate::ToolRegistry| {
            registry
                .definitions()
                .iter()
                .any(|tool| tool.name == "present_svg")
        };
        assert!(has(crate::ToolRegistry::direct(Vec::new())));
        assert!(has(crate::ToolRegistry::for_subagent_work()));
        assert!(!has(crate::ToolRegistry::coordinator(Vec::new(), None)));
        assert!(!has(crate::ToolRegistry::for_subagent_explore_no_sandbox()));
        assert!(!has(
            crate::ToolRegistry::for_subagent_explore_with_sandbox()
        ));
    }
}
