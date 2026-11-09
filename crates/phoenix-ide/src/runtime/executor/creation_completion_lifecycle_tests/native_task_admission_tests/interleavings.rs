use super::*;
use crate::runtime::task_approval_admission::{
    NativeTaskApprovalOutcome, NativeTaskApprovalRefusal,
};
use crate::runtime::traits::{DatabaseStorage, RegistryLlmClient};
use std::collections::BTreeMap;
use std::path::Path;

type NativeRuntime = ConversationRuntime<DatabaseStorage, RegistryLlmClient, Arc<MockToolExecutor>>;

struct ReleaseOnDrop(Option<std::sync::mpsc::Sender<()>>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
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
#[allow(clippy::too_many_lines)]
async fn production_initial_source_cancel_at_pre_git_refuses_without_approval_effects() {
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
    let (release_tx, release) = std::sync::mpsc::channel();
    *target.native_pre_git_gate.lock().unwrap() = Some(Box::new(move || {
        let _ = entered_tx.send(());
        // This closure is consumed by the real Initial Git blocking worker, not an async task.
        let _ = release.recv();
    }));
    let release = ReleaseOnDrop(Some(release_tx));
    let mut approval = Box::pin(target.execute_native_task_approval(command.clone()));
    let cancel_source = async {
        let _release = release;
        entered.await.map_err(|error| error.to_string())?;
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
        Ok::<(), String>(())
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
    cancellation.expect("real acknowledged source cancellation precedes pre-Git proof");
    assert_eq!(
        outcome,
        NativeTaskApprovalOutcome::NotApplied(NativeTaskApprovalRefusal::BeforeGitRefused)
    );
    assert!(target.native_pre_git_gate.lock().unwrap().is_none());
    assert!(!target.fatal_local_authority_fence.is_closed());
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
    assert_eq!(git_snapshot(&root, &worktree), before_git);
    assert_eq!(worktree_files(&worktree), before_files);
    assert_eq!(
        std::fs::read(worktree.join(task_file)).unwrap(),
        BODY.as_bytes()
    );
}

fn git_snapshot(root: &Path, worktree: &Path) -> Vec<String> {
    vec![
        git(root, &["rev-parse", "HEAD"]),
        git(root, &["show-ref"]),
        git(root, &["worktree", "list", "--porcelain"]),
        git(root, &["status", "--porcelain=v1", "--untracked-files=all"]),
        git(worktree, &["rev-parse", "HEAD"]),
        git(worktree, &["symbolic-ref", "HEAD"]),
        git(
            worktree,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        ),
        git(worktree, &["diff", "--binary", "HEAD"]),
    ]
}

fn worktree_files(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                files.insert(relative, None);
                visit(root, &path, files);
            } else {
                files.insert(relative, Some(std::fs::read(path).unwrap()));
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}
