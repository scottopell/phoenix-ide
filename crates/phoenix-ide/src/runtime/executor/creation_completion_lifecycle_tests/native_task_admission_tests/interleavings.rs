use super::*;
use crate::runtime::task_approval_admission::{
    NativeTaskApprovalOutcome, NativeTaskApprovalRefusal,
};
use crate::runtime::traits::{DatabaseStorage, RegistryLlmClient};
use std::collections::BTreeMap;
use std::path::Path;

type NativeRuntime = ConversationRuntime<DatabaseStorage, RegistryLlmClient, Arc<MockToolExecutor>>;

enum ReleaseOnDrop {
    Blocking(Option<std::sync::mpsc::Sender<()>>),
    Async(Option<oneshot::Sender<()>>),
}

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        match self {
            Self::Blocking(release) => {
                if let Some(release) = release.take() {
                    let _ = release.send(());
                }
            }
            Self::Async(release) => {
                if let Some(release) = release.take() {
                    let _ = release.send(());
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CancellationStage {
    BeforeGit,
    AfterGit,
}

fn native_runtime(
    context: ConvContext,
    state: ConvState,
    db: &Database,
    models: Arc<ModelRegistry>,
) -> NativeRuntime {
    let (event_tx, event_rx) = mpsc::channel(4);
    ConversationRuntime::new(
        context,
        state,
        DatabaseStorage::new(db.clone()),
        RegistryLlmClient::new(models.clone(), "claude-sonnet-5".into()),
        Arc::new(MockToolExecutor::new()),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        models,
        crate::terminal::ActiveTerminals::new(),
        event_rx,
        event_tx,
        crate::runtime::SseBroadcaster::new(128, 0),
    )
    .with_native_task_admission_database(db.clone())
}

#[tokio::test]
async fn production_initial_source_cancel_at_pre_git_refuses_without_approval_effects() {
    Box::pin(run_initial_source_cancel_interleaving(
        CancellationStage::BeforeGit,
    ))
    .await;
}

#[tokio::test]
async fn production_initial_source_cancel_after_git_fails_stop_before_sqlite() {
    Box::pin(run_initial_source_cancel_interleaving(
        CancellationStage::AfterGit,
    ))
    .await;
}

#[allow(clippy::too_many_lines)]
async fn run_initial_source_cancel_interleaving(stage: CancellationStage) {
    use super::super::super::test_git_helpers::{add_explore_worktree, init_repo};
    use phoenix_db::conversation_goals::control::GoalControlInstallation;
    use phoenix_db::native_task_approval::NativeTaskApprovalInspection;

    let (_temp, root) = init_repo();
    let worktree = add_explore_worktree(&root, "actor-target", "main");
    let task_file = "tasks/reviewed.md";
    std::fs::create_dir_all(worktree.join("tasks")).unwrap();
    std::fs::write(worktree.join(task_file), BODY).unwrap();
    let f = AdmissionFixture::with_cwd_and_task_file("/nonexistent", task_file).await;
    assert_eq!(
        f.db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    f.db.update_conversation_mode_and_cwd(
        &f.target,
        &ConvMode::Explore {
            worktree_path: Some(
                NonEmptyString::new(worktree.to_string_lossy().into_owned()).unwrap(),
            ),
            next_taskmd_id_hint: None,
        },
        worktree.to_str().unwrap(),
    )
    .await
    .unwrap();
    let InitialSource {
        source, command, ..
    } = initial_source(&f, task_file, ReviewedTaskKind::PlainMarkdown).await;
    assert!(matches!(
        f.db.inspect_native_task_approval_command(&command)
            .await
            .unwrap(),
        NativeTaskApprovalInspection::RequiresOwningActorAdmission
    ));
    let current_coordinator = f.db.coordinator_conversation_id().await.unwrap();
    assert_eq!(current_coordinator.as_deref(), Some(source.id.as_str()));

    let (request_tx, mut requests) = mpsc::unbounded_channel();
    let llm = Arc::new(NativeApprovalLlm {
        calls: std::sync::atomic::AtomicUsize::new(0),
        requests: request_tx,
    });
    let models = Arc::new(ModelRegistry::for_test_with_sonnet(llm.clone()));
    let before_target = f.db.get_conversation(&f.target).await.unwrap();
    let authority = crate::resource_authority::resolve_resource_authority(&f.db, &before_target)
        .await
        .unwrap();
    let mut target_context = ConvContext::new(
        &f.target,
        worktree.clone(),
        "claude-sonnet-5",
        200_000,
        16_384,
    );
    target_context.resource_scope = authority.scope;
    target_context.resource_authority = authority.authority;
    target_context.work_scope_worktree = Some(worktree.clone());
    target_context.desired_base_branch = Some("main".into());
    let mut target = native_runtime(
        target_context,
        before_target.state.clone(),
        &f.db,
        models.clone(),
    );
    assert!(matches!(
        target.resolve_task_approval_execution().await.unwrap(),
        TaskApprovalExecution::Initial
    ));
    let mut target_events = target.broadcast_tx.subscribe();
    let before_scope =
        f.db.get_conversation_work_scope_context(&f.target)
            .await
            .unwrap();
    let before_rows = native_approval_rows(&f.db).await;
    let before_source_calls =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM native_task_approval_source_calls")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(&before_rows[1..], &[0, 0, 0, 0]);
    let before_messages =
        serde_json::to_value(f.db.get_messages(&f.target).await.unwrap()).unwrap();
    let before_git = git_snapshot(&root, &worktree);
    let before_files = worktree_files(&worktree);

    // Construct directly from durable ToolExecuting; run() would recover an interrupted tool.
    let persisted_source = f.db.get_conversation(&source.id).await.unwrap();
    assert!(matches!(
        persisted_source.state,
        ConvState::ToolExecuting { .. }
    ));
    let mut source_runtime = native_runtime(
        ConvContext::coordinator(&source.id, "claude-sonnet-5", 200_000, 16_384),
        persisted_source.state,
        &f.db,
        models,
    );
    // No local tool was dispatched: AbortTool's empty execution slot is intentionally a no-op.
    assert_eq!(source_runtime.tool_execution.generation(), 0);
    assert!(source_runtime
        .tool_executor
        .recorded_executions()
        .is_empty());

    let (entered_tx, entered) = oneshot::channel();
    let release = match stage {
        CancellationStage::BeforeGit => {
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            *target.native_pre_git_gate.lock().unwrap() = Some(Box::new(move || {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
            }));
            ReleaseOnDrop::Blocking(Some(release_tx))
        }
        CancellationStage::AfterGit => {
            let (release_tx, release_rx) = oneshot::channel();
            target.native_post_git_gate = Some((entered_tx, release_rx));
            ReleaseOnDrop::Async(Some(release_tx))
        }
    };
    let mut approval = Box::pin(target.execute_native_task_approval(command.clone()));
    let cancel_source = async {
        let _release = release;
        entered.await.map_err(|error| error.to_string())?;
        let post_git = if stage == CancellationStage::AfterGit {
            let snapshot = try_git_snapshot(&root, &worktree)?;
            let files = try_worktree_files(&worktree)?;
            let expected_branch = phoenix_core::task_source::TaskSource::detect("reviewed.md")
                .ok_or_else(|| "reviewed task filename has no TaskSource".to_owned())?
                .branch_and_id(&f.target)
                .0;
            let actual_branch = try_git(&worktree, &["branch", "--show-current"])?;
            let committed_body = try_git(&worktree, &["show", "HEAD:tasks/reviewed.md"])?;
            let worktree_body =
                std::fs::read(worktree.join(task_file)).map_err(|error| error.to_string())?;
            if snapshot[4] == before_git[4]
                || actual_branch.trim_end() != expected_branch
                || committed_body != BODY
                || worktree_body != BODY.as_bytes()
            {
                return Err(
                    "post-Git gate must observe the real committed task and branch".to_owned(),
                );
            }
            Some((snapshot, files))
        } else {
            None
        };
        let acknowledged = source_runtime
            .process_acknowledged_event(Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested,
            })
            .await?;
        let stored =
            f.db.get_conversation(&source.id)
                .await
                .map_err(|error| error.to_string())?;
        let head =
            f.db.coordinator_conversation_id()
                .await
                .map_err(|error| error.to_string())?;
        let finalized =
            f.db.message_exists(&command.source.message_id)
                .await
                .map_err(|error| error.to_string())?;
        if acknowledged != AcknowledgedEventOutcome::Settled
            || stored.state != source_runtime.state
            || !matches!(&stored.state, ConvState::CancellingTool { tool_use_id, cause, .. }
                if tool_use_id == &command.source.tool_use_id && *cause == CancelCause::UserRequested)
            || head != current_coordinator
            || finalized
        {
            return Err(
                "source cancellation must settle durably without rotating or finalizing the source"
                    .to_owned(),
            );
        }
        Ok::<_, String>(post_git)
        // Dropping _release permits the unchanged real DB source/current-Coordinator proof.
    };
    let joined = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(&mut approval, cancel_source)
    })
    .await;
    let (outcome, cancellation) = match joined {
        Ok(results) => results,
        Err(error) => {
            // The timed-out schedule drops its release guard; retain and drain the real worker.
            let _ = approval.as_mut().await;
            panic!("pre-Git cancellation schedule timed out after worker drain: {error}");
        }
    };
    drop(approval);
    let post_git = cancellation.expect("real acknowledged source cancellation at Git boundary");
    match stage {
        CancellationStage::BeforeGit => assert_eq!(
            outcome,
            NativeTaskApprovalOutcome::NotApplied(NativeTaskApprovalRefusal::BeforeGitRefused)
        ),
        CancellationStage::AfterGit => {
            assert_eq!(outcome, NativeTaskApprovalOutcome::Unavailable);
            assert!(target.native_post_git_gate.is_none());
            assert!(target.fatal_local_authority_fence.is_closed());
            assert_eq!(
                target.recovery_disposition,
                RuntimeRecoveryDisposition::RecreateFromDatabase
            );
        }
    }
    assert!(target.native_pre_git_gate.lock().unwrap().is_none());
    if stage == CancellationStage::BeforeGit {
        assert!(!target.fatal_local_authority_fence.is_closed());
    }
    assert_eq!(source_runtime.tool_execution.generation(), 0);
    assert!(source_runtime
        .tool_executor
        .recorded_executions()
        .is_empty());
    assert!(target.tool_executor.recorded_executions().is_empty());
    assert!(target.llm_task_handle.is_none());
    assert!(source_runtime.llm_task_handle.is_none());
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert!(requests.try_recv().is_err());
    assert!(
        matches!(
            target_events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "no target approval message, Work publication or provider-state publication"
    );
    assert_eq!(native_approval_rows(&f.db).await, before_rows);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM native_task_approval_source_calls")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        before_source_calls,
        "source-call rows remain unchanged independently of approval artifact counts"
    );
    assert_eq!(
        serde_json::to_value(f.db.get_messages(&f.target).await.unwrap()).unwrap(),
        before_messages
    );
    let after_target = f.db.get_conversation(&f.target).await.unwrap();
    assert_eq!(after_target.state, before_target.state);
    assert_eq!(target.state, before_target.state);
    assert_eq!(after_target.conv_mode, before_target.conv_mode);
    assert_eq!(after_target.cwd, before_target.cwd);
    assert_eq!(
        after_target.attached_work_scope_id,
        before_target.attached_work_scope_id
    );
    assert_eq!(
        f.db.get_conversation_work_scope_context(&f.target)
            .await
            .unwrap(),
        before_scope
    );
    assert_eq!(
        target.context.resource_authority,
        crate::work_scope::ResourceAuthority::Restricted
    );
    assert!(f
        .db
        .get_approved_task_objective(&f.target)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.db.coordinator_conversation_id().await.unwrap(),
        current_coordinator
    );
    assert_eq!(
        f.db.get_conversation(&source.id).await.unwrap().state,
        source_runtime.state
    );
    assert!(!f
        .db
        .message_exists(&command.source.message_id)
        .await
        .unwrap());
    match (stage, post_git) {
        (CancellationStage::BeforeGit, None) => {
            assert_eq!(git_snapshot(&root, &worktree), before_git);
            assert_eq!(worktree_files(&worktree), before_files);
        }
        (CancellationStage::AfterGit, Some((post_git_snapshot, post_git_files))) => {
            assert_ne!(post_git_snapshot, before_git);
            assert_eq!(git_snapshot(&root, &worktree), post_git_snapshot);
            assert_eq!(worktree_files(&worktree), post_git_files);
            assert_eq!(git(&worktree, &["show", "HEAD:tasks/reviewed.md"]), BODY);
        }
        (CancellationStage::BeforeGit, Some(_)) | (CancellationStage::AfterGit, None) => {
            panic!("Git snapshot matches the configured interleaving");
        }
    }
}

fn try_git_snapshot(root: &Path, worktree: &Path) -> Result<Vec<String>, String> {
    Ok(vec![
        try_git(root, &["rev-parse", "HEAD"])?,
        try_git(root, &["show-ref"])?,
        try_git(root, &["worktree", "list", "--porcelain"])?,
        try_git(root, &["status", "--porcelain=v1", "--untracked-files=all"])?,
        try_git(worktree, &["rev-parse", "HEAD"])?,
        try_git(worktree, &["symbolic-ref", "HEAD"])?,
        try_git(
            worktree,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )?,
        try_git(worktree, &["diff", "--binary", "HEAD"])?,
    ])
}

fn try_git(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = phoenix_core::git::command()
        .current_dir(path)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}

fn try_worktree_files(root: &Path) -> Result<BTreeMap<PathBuf, Option<Vec<u8>>>, String> {
    fn visit(
        root: &Path,
        dir: &Path,
        files: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_path_buf();
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                files.insert(relative, None);
                visit(root, &path, files)?;
            } else {
                files.insert(
                    relative,
                    Some(std::fs::read(path).map_err(|error| error.to_string())?),
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn git_snapshot(root: &Path, worktree: &Path) -> Vec<String> {
    try_git_snapshot(root, worktree).expect("Git snapshot outside the paused approval schedule")
}

fn worktree_files(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    try_worktree_files(root).expect("filesystem snapshot outside the paused approval schedule")
}
