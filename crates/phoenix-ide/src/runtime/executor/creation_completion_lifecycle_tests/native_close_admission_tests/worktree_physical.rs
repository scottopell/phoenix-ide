#![cfg(target_os = "linux")]

//! Public native Close qualification; all Git and SQLite state is disposable.
//! REQ-WL-002/002f: original Worktree and independent WorkScope retirement.

use super::*;
use phoenix_core::domain::close::{
    CapturedWorktreeIdentity, CloseAttemptId, CloseCompletionOutcome, ClosePhase,
    CloseRetirementSnapshot, CloseRunRef, CloseRunStatus, GitPathIdentity, LossItemIdentity,
    OpaqueIdentity, RetiredResourceKind, RetirementOutcome, WorktreeFingerprint, WorktreeId,
    WorktreeIdentity,
};
use phoenix_core::domain::db_schema::NonEmptyString;
use phoenix_core::domain::product_conversation::OrdinaryProductConversationLifecycle;
use phoenix_core::work_scope::{WorkScopeId, WorkScopeLifecycle};
use serde_json::Value;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[tokio::test]
async fn registered_native_close_physically_retires_worktree_only_and_replays_without_effects() {
    run_public_worktree_close(WorktreeJourney::Retire).await;
}

#[tokio::test]
async fn registered_native_close_after_acknowledged_source_stop_preserves_real_worktree() {
    run_public_worktree_close(WorktreeJourney::StopBeforeAdmission).await;
}

#[tokio::test]
async fn registered_native_close_after_source_stop_preserves_reused_path_and_original_checkout() {
    run_public_worktree_close(WorktreeJourney::StopBeforeAdmissionWithReusedPath).await;
}

#[derive(Clone, Copy)]
enum WorktreeJourney {
    Retire,
    StopBeforeAdmission,
    StopBeforeAdmissionWithReusedPath,
}

struct ReusedWorktreePath {
    retained_original: PathBuf,
    replacement_admin: PathBuf,
    replacement_fingerprint: String,
}

fn directory_identity(path: &Path) -> (u64, u64) {
    let metadata = std::fs::symlink_metadata(path).unwrap();
    assert!(
        metadata.is_dir(),
        "{} must be an actual directory",
        path.display()
    );
    (metadata.dev(), metadata.ino())
}

#[derive(Debug, PartialEq, Eq)]
struct PhysicalEntry {
    path: PathBuf,
    device: u64,
    inode: u64,
    mode: u32,
    content: Vec<u8>,
}

// No timestamps: ordinary reads may change atime without changing an object.
fn physical_snapshot(root: &Path) -> Vec<PhysicalEntry> {
    fn capture(root: &Path, path: &Path, entries: &mut Vec<PhysicalEntry>) {
        let metadata = std::fs::symlink_metadata(path).unwrap();
        let content = if metadata.is_file() {
            std::fs::read(path).unwrap()
        } else if metadata.file_type().is_symlink() {
            std::fs::read_link(path)
                .unwrap()
                .as_os_str()
                .as_bytes()
                .to_vec()
        } else {
            assert!(
                metadata.is_dir(),
                "fixture contains only directories, files and links"
            );
            Vec::new()
        };
        entries.push(PhysicalEntry {
            path: path.strip_prefix(root).unwrap().to_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            content,
        });
        if metadata.is_dir() {
            let mut children: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            children.sort();
            for child in children {
                capture(root, &child, entries);
            }
        }
    }
    let mut entries = Vec::new();
    capture(root, root, &mut entries);
    entries
}

struct DisposableGit {
    _temp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    worktree: PathBuf,
    admin: PathBuf,
    sibling: PathBuf,
    sibling_admin: PathBuf,
    fingerprint: String,
    checkout_identity: (u64, u64),
    admin_identity: (u64, u64),
    original_refs: Vec<u8>,
    original_registrations: Vec<u8>,
}

impl DisposableGit {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("home")).unwrap();
        std::fs::create_dir(root.join("empty-template")).unwrap();
        let repo = root.join("main repository");
        std::fs::create_dir(&repo).unwrap();
        let worktree = root.join("original linked Worktree");
        let sibling = root.join("unrelated linked Worktree");
        let mut git = Self {
            _temp: temp,
            root,
            repo,
            worktree,
            admin: PathBuf::new(),
            sibling,
            sibling_admin: PathBuf::new(),
            fingerprint: String::new(),
            checkout_identity: (0, 0),
            admin_identity: (0, 0),
            original_refs: Vec::new(),
            original_registrations: Vec::new(),
        };
        git.run(
            &git.repo,
            &[
                "init",
                "--initial-branch=main",
                "--template=../empty-template",
            ],
        );
        for (key, value) in [
            ("user.name", "Disposable Close Fixture"),
            ("user.email", "close-fixture@example.invalid"),
            ("commit.gpgSign", "false"),
            ("core.fsmonitor", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git.run(&git.repo, &["config", "--local", key, value]);
        }
        std::fs::write(git.repo.join("tracked.txt"), b"durable content\n").unwrap();
        git.run(&git.repo, &["add", "--", "tracked.txt"]);
        git.run(&git.repo, &["commit", "-m", "Disposable baseline"]);
        git.run(
            &git.repo,
            &[
                "worktree",
                "add",
                "-b",
                "disposable-close",
                git.worktree.to_str().unwrap(),
                "main",
            ],
        );
        git.run(
            &git.repo,
            &[
                "worktree",
                "add",
                "-b",
                "untargeted-sibling",
                git.sibling.to_str().unwrap(),
                "main",
            ],
        );
        git.worktree = git.worktree.canonicalize().unwrap();
        git.sibling = git.sibling.canonicalize().unwrap();
        git.admin = git.administrative_directory(&git.worktree);
        git.sibling_admin = git.administrative_directory(&git.sibling);
        assert_ne!(git.admin, git.sibling_admin);
        assert!(git.admin.starts_with(git.repo.join(".git/worktrees")));
        assert!(git
            .sibling_admin
            .starts_with(git.repo.join(".git/worktrees")));
        git.checkout_identity = directory_identity(&git.worktree);
        git.admin_identity = directory_identity(&git.admin);
        assert_ne!(git.checkout_identity, directory_identity(&git.sibling));
        assert_ne!(git.admin_identity, directory_identity(&git.sibling_admin));
        assert_eq!(
            std::fs::read(git.admin.join("gitdir")).unwrap(),
            format!("{}\n", git.worktree.join(".git").display()).into_bytes()
        );
        git.fingerprint = phoenix_core::git::observe_linked_worktree_fingerprint(&git.worktree)
            .expect("real linked Worktree has its original administrative fingerprint");
        assert!(git
            .run(&git.worktree, &["status", "--porcelain=v1", "--ignored"])
            .is_empty());
        git.original_refs = git.run(
            &git.repo,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        git.original_registrations = git.run(&git.repo, &["worktree", "list", "--porcelain", "-z"]);
        git
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Vec<u8> {
        assert!(
            cwd.starts_with(&self.root),
            "Git never targets a user's repository"
        );
        let output = Command::new("git")
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("Git is available on PATH"),
            )
            .env("HOME", self.root.join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .current_dir(cwd)
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .output()
            .expect("disposable Git fixture command starts");
        assert!(
            output.status.success(),
            "disposable Git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn administrative_directory(&self, worktree: &Path) -> PathBuf {
        let bytes = self.run(worktree, &["rev-parse", "--absolute-git-dir"]);
        let text = std::str::from_utf8(&bytes).unwrap();
        PathBuf::from(text.strip_suffix('\n').unwrap())
            .canonicalize()
            .unwrap()
    }

    async fn register(&self, fixture: &CloseFixture) -> (WorkScopeId, WorktreeIdentity) {
        let scope = fixture
            .db
            .get_conversation(TARGET_ID)
            .await
            .unwrap()
            .attached_work_scope_id
            .expect("target already owns its original WorkScope");
        // Use the same ConvMode -> allocated_worktree conversion as normal Work
        // registration, rather than inventing a scope or injecting its identity.
        let mode = phoenix_core::domain::db_schema::ConvMode::Work {
            branch_name: NonEmptyString::new("disposable-close").unwrap(),
            worktree_path: NonEmptyString::new(self.worktree.to_str().unwrap()).unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("disposable-close-fixture").unwrap(),
            task_title: NonEmptyString::new("Disposable Worktree Close qualification").unwrap(),
        };
        fixture
            .db
            .update_conversation_mode_and_cwd(TARGET_ID, &mode, self.worktree.to_str().unwrap())
            .await
            .unwrap();
        let (kind, id, fingerprint): (String, String, String) = sqlx::query_as(
            "SELECT environment_kind, worktree_id, worktree_fingerprint
             FROM work_scopes WHERE id = ?1",
        )
        .bind(scope.as_str())
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
        assert_eq!(kind, "allocated_worktree");
        assert_eq!(fingerprint, self.fingerprint);
        assert_eq!(
            fixture
                .db
                .get_conversation(TARGET_ID)
                .await
                .unwrap()
                .attached_work_scope_id
                .as_ref(),
            Some(&scope)
        );
        assert_eq!(
            fixture
                .db
                .get_conversation_work_scope_context(TARGET_ID)
                .await
                .unwrap()
                .1,
            WorkScopeLifecycle::Active
        );
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse(id).unwrap(),
            WorktreeFingerprint::parse(fingerprint).unwrap(),
            GitPathIdentity::from_bytes(self.worktree.as_os_str().as_bytes().to_vec()),
        );
        (scope, identity)
    }

    fn reuse_original_path_after_stop(&mut self) -> ReusedWorktreePath {
        let retained_original = self.root.join("original checkout retained after Stop");
        self.run(
            &self.repo,
            &[
                "worktree",
                "move",
                self.worktree.to_str().unwrap(),
                retained_original.to_str().unwrap(),
            ],
        );
        self.run(
            &self.repo,
            &[
                "worktree",
                "add",
                "-b",
                "replacement-after-stop",
                self.worktree.to_str().unwrap(),
                "main",
            ],
        );
        let replacement_admin = self.administrative_directory(&self.worktree);
        let replacement_fingerprint =
            phoenix_core::git::observe_linked_worktree_fingerprint(&self.worktree).unwrap();
        assert_ne!(replacement_admin, self.admin);
        assert_ne!(replacement_fingerprint, self.fingerprint);
        assert_ne!(directory_identity(&self.worktree), self.checkout_identity);
        assert_eq!(
            directory_identity(&retained_original),
            self.checkout_identity
        );
        assert_eq!(directory_identity(&self.admin), self.admin_identity);
        assert_eq!(
            self.administrative_directory(&retained_original),
            self.admin
        );
        self.original_refs = self.run(
            &self.repo,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        ReusedWorktreePath {
            retained_original,
            replacement_admin,
            replacement_fingerprint,
        }
    }

    fn assert_untargeted_state_preserved(&self) {
        assert_eq!(
            self.run(
                &self.repo,
                &["for-each-ref", "--format=%(refname) %(objectname)"]
            ),
            self.original_refs
        );
        assert_eq!(
            std::fs::read(self.repo.join("tracked.txt")).unwrap(),
            b"durable content\n"
        );
        assert_eq!(
            std::fs::read(self.sibling.join("tracked.txt")).unwrap(),
            b"durable content\n"
        );
        assert_eq!(
            self.administrative_directory(&self.sibling),
            self.sibling_admin
        );
        assert!(self.sibling_admin.join("gitdir").is_file());
    }
}

async fn run_public_worktree_close(journey: WorktreeJourney) {
    use crate::coordinator_tools;
    use crate::send_chat_service::SendChatApplicationService;
    use tokio_util::sync::CancellationToken;

    let mut git = DisposableGit::new();
    let stop_before_admission = !matches!(journey, WorktreeJourney::Retire);
    let fixture = CloseFixture::new().await;
    let (scope, original_worktree) = git.register(&fixture).await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let retriever = Arc::new(fixture.db.fts_retriever());
    let read_service =
        crate::api::global_read::GlobalReadService::new(fixture.db.clone(), retriever);
    let send_chat = Arc::new(SendChatApplicationService::new(
        fixture.db.clone(),
        Arc::clone(&backend),
    ));
    let registered_tools = coordinator_tools::tools(
        read_service,
        send_chat,
        crate::api::work_scope_load::WorkScopeLoadService::new(
            backend.bash_handles(),
            crate::api::resource_monitor::ResourceMonitor::new(),
            backend.terminals.clone(),
        ),
        &fixture.db,
    );
    let tool = registered_tools
        .into_iter()
        .find(|tool| tool.name() == "close_conversation")
        .expect("the coordinator registration must include close_conversation");
    let (event_tx, event_rx) = mpsc::channel(32);
    let mut actor = ConversationRuntime::new(
        ConvContext::coordinator(&fixture.source_transcript_id, "test-model", 200_000, 16_384),
        fixture.state.clone(),
        Arc::new(crate::runtime::traits::DatabaseStorage::new(
            fixture.db.clone(),
        )),
        Arc::new(MockLlmClient::new("test-model")),
        Arc::new(MockToolExecutor::new()),
        backend.browser_sessions().clone(),
        backend.bash_handles().clone(),
        backend.tmux_registry().clone(),
        Arc::new(ModelRegistry::new_empty()),
        backend.terminals.clone(),
        event_rx,
        event_tx.clone(),
        crate::runtime::SseBroadcaster::new(128, 0),
    )
    .with_fatal_local_authority_fence(Arc::clone(&backend.fatal_local_authority_fence))
    .with_native_task_admission_database(fixture.db.clone());

    let (sender, receiver) = mpsc::channel(4);
    let (watch_tx, state_rx) = tokio::sync::watch::channel(fixture.state.clone());
    let (startup, started) = oneshot::channel();
    actor = actor
        .with_state_watcher(watch_tx)
        .with_acknowledged_event_receiver(receiver)
        .with_startup_llm_recovery_ack(startup);
    backend.runtimes.write().await.insert(
        fixture.source_transcript_id.clone(),
        crate::runtime::ConversationHandle {
            event_tx: event_tx.clone(),
            acknowledged_event_tx: sender.clone(),
            handoff_review_tx: None,
            turn_trigger: crate::runtime::TurnTriggerSlot::default(),
            broadcast_tx: crate::runtime::SseBroadcaster::new(128, 0),
            identity: Arc::new(()),
            state_rx,
            startup_llm_recovery_ack: Arc::new(Mutex::new(None)),
        },
    );
    let running = tokio::spawn(actor.run());
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .expect("source actor must finish startup")
        .unwrap()
        .unwrap();
    if stop_before_admission {
        stop_actor(&sender).await;
    }
    let reused_path = if matches!(journey, WorktreeJourney::StopBeforeAdmissionWithReusedPath) {
        // The Stop ACK above is the causal fence. All mutation finishes before
        // public ingress; no race against inventory capture or worker dispatch.
        Some(git.reuse_original_path_after_stop())
    } else {
        None
    };
    // Intentional move/add setup changes registrations. Rejection must preserve
    // this post-setup listing, not the original pre-setup Worktree paths.
    let registrations_before_admission =
        git.run(&git.repo, &["worktree", "list", "--porcelain", "-z"]);
    let before_admission = durable_target_rows(&fixture).await;
    let physical_before_admission = physical_snapshot(&git.root);
    let sibling_before_admission = physical_snapshot(&git.sibling);
    let sibling_admin_before_admission = physical_snapshot(&git.sibling_admin);
    assert_eq!(close_row_counts(&fixture).await, vec![0; 9]);
    let context = crate::tools::ToolContext::new(
        CancellationToken::new(),
        fixture.source_transcript_id.clone(),
        git.root.clone(),
        backend.browser_sessions().clone(),
        backend.bash_handles().clone(),
        Arc::new(ModelRegistry::new_empty()),
        backend.terminals.clone(),
        backend.tmux_registry().clone(),
        None,
        WorkScopeId::parse("close-source-test").unwrap(),
    )
    .with_tool_use_id(fixture.source.tool_use_id.clone())
    .with_svg_assistant_message_id(fixture.source.message_id.clone());
    let input = serde_json::json!({
        "action": "initial",
        "product_conversation_id": fixture.target.as_str()
    });
    // This call crosses registered coordinator ingress, application service,
    // RuntimeManager source admission, and the real actor's run_inner loop.
    // No worker, private Git adapter, alternate delete, or process is supplied.
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tool.run(input.clone(), context.clone()),
    )
    .await
    .expect("public native Close must return a bounded admission result");
    if stop_before_admission {
        assert!(
            !output.is_success(),
            "an acknowledged Stop cannot admit Close"
        );
        let rejection: Value = serde_json::from_str(&output.output()).unwrap();
        assert_eq!(rejection["code"], "source_refused");
        assert_eq!(close_row_counts(&fixture).await, vec![0; 9]);
        assert_eq!(durable_target_rows(&fixture).await, before_admission);
        assert_eq!(
            fixture
                .db
                .get_conversation_work_scope_context(TARGET_ID)
                .await
                .unwrap()
                .1,
            WorkScopeLifecycle::Active
        );
        assert!(
            !fixture
                .db
                .get_conversation(TARGET_ID)
                .await
                .unwrap()
                .archived
        );
        assert_eq!(physical_snapshot(&git.root), physical_before_admission);
        assert_eq!(directory_identity(&git.admin), git.admin_identity);
        if let Some(reused) = &reused_path {
            assert_eq!(
                directory_identity(&reused.retained_original),
                git.checkout_identity
            );
            assert_eq!(
                git.administrative_directory(&reused.retained_original),
                git.admin
            );
            assert_eq!(
                git.administrative_directory(&git.worktree),
                reused.replacement_admin
            );
            assert_eq!(
                phoenix_core::git::observe_linked_worktree_fingerprint(&git.worktree).as_deref(),
                Some(reused.replacement_fingerprint.as_str())
            );
        } else {
            assert_eq!(directory_identity(&git.worktree), git.checkout_identity);
            assert_eq!(
                phoenix_core::git::observe_linked_worktree_fingerprint(&git.worktree).as_deref(),
                Some(git.fingerprint.as_str())
            );
            assert_eq!(git.administrative_directory(&git.worktree), git.admin);
        }
        assert_eq!(
            std::fs::read(git.worktree.join("tracked.txt")).unwrap(),
            b"durable content\n"
        );
        assert_eq!(
            git.run(&git.repo, &["worktree", "list", "--porcelain", "-z"]),
            registrations_before_admission
        );
        git.assert_untargeted_state_preserved();
    } else {
        assert!(output.is_success(), "{}", output.output());
        let admitted: Value = serde_json::from_str(&output.output()).unwrap();
        assert_eq!(admitted["status"], "newly_admitted");
        assert_eq!(admitted["admitted"]["product_conversation_id"], TARGET_ID);
        assert_eq!(admitted["admitted"]["run_ordinal"], 1);
        assert!(
            admitted.get("closed").is_none(),
            "admission must not claim completion"
        );
        let attempt =
            CloseAttemptId::parse(admitted["admitted"]["attempt_id"].as_str().unwrap()).unwrap();
        let run = CloseRunRef::initial(attempt.clone());
        // The registered tool serializes only an admission receipt, but the
        // newly admitted actor continuation retains its response sender until
        // release_if_exactly_terminal observes this exact run and obligation.
        // Thus the awaited response is the terminal fence; the actor's terminal
        // observation path may release it without a Finished phase notification.
        // Public observe reads retained state; it is not a completion barrier.
        let obligation = fixture
            .db
            .get_close_obligation(attempt.as_str())
            .await
            .unwrap();
        assert_eq!(obligation.phase(), ClosePhase::Completed);
        assert_eq!(obligation.attempt_id(), &attempt);
        assert_eq!(obligation.product_conversation_id(), &fixture.target);
        let receipt: (String, String, String, String, String, String, String, i64, String) =
            sqlx::query_as(
                "SELECT request_id, source_transcript_id, source_message_id, source_tool_use_id,
                        request_fingerprint, product_conversation_id, attempt_id, run_ordinal, intent
                 FROM native_close_initial_receipts",
            )
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
        assert_eq!(receipt.0, fixture.command.request_id().to_string());
        assert_eq!(receipt.0, admitted["request_id"].as_str().unwrap());
        assert_eq!(receipt.1, fixture.source_transcript_id);
        assert_eq!(receipt.2, fixture.source.message_id);
        assert_eq!(receipt.3, fixture.source.tool_use_id);
        assert_eq!(receipt.4, fixture.command.fingerprint());
        assert_eq!(receipt.5, TARGET_ID);
        assert_eq!(receipt.6, attempt.as_str());
        assert_eq!(receipt.7, 1);
        assert_eq!(receipt.8, "initial");
        assert_eq!(
            obligation.close_outcome(),
            Some(CloseCompletionOutcome::Archived)
        );
        assert_eq!(
            fixture.db.get_close_run(&run).await.unwrap().status,
            CloseRunStatus::Completed
        );
        let snapshot = obligation
            .snapshot()
            .cloned()
            .expect("physical completion retains its authorized retirement snapshot");
        // The original anchor is available after inventory capture, not at
        // admission. Completion and both independent resources must use it.
        let (generation, fingerprint): (String, String) = sqlx::query_as(
            "SELECT inspection_generation, inspection_fingerprint
             FROM close_original_retirement_anchors WHERE attempt_id = ?1",
        )
        .bind(attempt.as_str())
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
        assert_eq!(
            snapshot,
            CloseRetirementSnapshot::parse(generation, fingerprint).unwrap()
        );
        let captured = fixture
            .db
            .list_close_attempt_scopes(attempt.as_str())
            .await
            .unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].scope, scope);
        assert_eq!(
            captured[0].captured_worktree,
            Some(CapturedWorktreeIdentity::Resolved(
                original_worktree.clone()
            ))
        );
        let expected = fixture
            .db
            .list_close_expected_retirement_resources(attempt.as_str())
            .await
            .unwrap();
        assert_eq!(
            expected.len(),
            2,
            "Worktree-only must not depend on process workers or coverage"
        );
        let worktrees: Vec<_> = expected
            .iter()
            .filter(|item| item.resource.kind() == RetiredResourceKind::Worktree)
            .collect();
        let scopes: Vec<_> = expected
            .iter()
            .filter(|item| item.resource.kind() == RetiredResourceKind::WorkScope)
            .collect();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(
            scopes.len(),
            1,
            "independent WorkScope is a distinct retirement resource"
        );
        assert_eq!(
            worktrees[0].resource.identity(),
            &LossItemIdentity::Worktree(original_worktree)
        );
        assert_eq!(
            scopes[0].resource.identity(),
            &LossItemIdentity::Opaque(OpaqueIdentity::parse(scope.as_str()).unwrap())
        );
        let cleanup_plan = fixture
            .db
            .close_worktree_cleanup_plan(&attempt, &scope, &snapshot, &worktrees[0].resource)
            .await
            .unwrap()
            .expect("physical Worktree removal retains its exact original admin plan");
        assert_eq!(cleanup_plan.administrative_dir, git.admin);
        assert_eq!(
            cleanup_plan.administrative_dir_incarnation,
            format!(
                "git_admin_dir_v1:{}:{}",
                git.admin_identity.0, git.admin_identity.1
            )
        );
        let tombstone = cleanup_plan
            .final_tombstone
            .as_ref()
            .expect("physical removal retains its descriptor-bound final object");
        assert_eq!(tombstone.object_device, Some(git.checkout_identity.0));
        assert_eq!(tombstone.object_inode, Some(git.checkout_identity.1));
        let outcomes = fixture
            .db
            .list_close_retirement_evidence(attempt.as_str())
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 2);
        for item in &expected {
            assert_eq!(item.attempt_id, attempt);
            assert_eq!(item.scope, scope);
            assert_eq!(item.snapshot, snapshot);
            assert!(fixture
                .db
                .close_retirement_resource_was_dispatched(
                    &attempt,
                    &scope,
                    &snapshot,
                    &item.resource,
                )
                .await
                .unwrap());
            let matching: Vec<_> = outcomes
                .iter()
                .filter(|outcome| outcome.resource == item.resource)
                .collect();
            assert_eq!(matching.len(), 1);
            assert_eq!(matching[0].scope, scope);
            assert_eq!(matching[0].attempt_id, attempt);
            assert_eq!(matching[0].snapshot, snapshot);
            assert_eq!(
                matching[0].outcome,
                RetirementOutcome::Retired,
                "physical success is not adopted absence"
            );
        }
        assert_eq!(
            fixture
                .db
                .get_conversation_work_scope_context(TARGET_ID)
                .await
                .unwrap()
                .1,
            WorkScopeLifecycle::Retired
        );
        // Do not let a dangling symlink masquerade as physical removal.
        assert_eq!(
            std::fs::symlink_metadata(&git.worktree).unwrap_err().kind(),
            std::io::ErrorKind::NotFound,
            "the actual original checkout must be removed"
        );
        assert_eq!(
            std::fs::symlink_metadata(&git.admin).unwrap_err().kind(),
            std::io::ErrorKind::NotFound,
            "the exact captured Git admin registration must be removed"
        );
        let registrations = git.run(&git.repo, &["worktree", "list", "--porcelain", "-z"]);
        let expected_registration = [
            b"worktree ".as_slice(),
            git.worktree.as_os_str().as_bytes(),
            b"\0",
        ]
        .concat();
        assert!(!registrations
            .windows(expected_registration.len())
            .any(|entry| entry == expected_registration));
        let original_listing = String::from_utf8(git.original_registrations.clone()).unwrap();
        let original_record_prefix = String::from_utf8(expected_registration).unwrap();
        let expected_remaining: Vec<_> = original_listing
            .split("\0\0")
            .filter(|record| !record.starts_with(&original_record_prefix))
            .collect();
        let actual_listing = String::from_utf8(registrations.clone()).unwrap();
        assert_eq!(
            actual_listing.split("\0\0").collect::<Vec<_>>(),
            expected_remaining,
            "only the exact original linked registration may disappear"
        );
        git.assert_untargeted_state_preserved();
        let target = fixture.db.get_conversation(TARGET_ID).await.unwrap();
        assert!(
            target.archived,
            "successful Close publishes Closed rather than only deleting resources"
        );
        assert_eq!(target.attached_work_scope_id.as_ref(), Some(&scope));
        assert_eq!(
            fixture
                .db
                .read_ordinary_product_conversation_metadata(TARGET_ID)
                .await
                .unwrap()
                .lifecycle,
            OrdinaryProductConversationLifecycle::History
        );
        let outcome_message_id = format!("close-outcome:{}", attempt.as_str());
        let messages = fixture.db.get_messages(TARGET_ID).await.unwrap();
        let close_messages: Vec<_> = messages
            .iter()
            .filter(|message| message.message_id == outcome_message_id)
            .collect();
        assert_eq!(close_messages.len(), 1);
        assert_eq!(
            messages.len(),
            1,
            "History contains exactly the Close outcome"
        );
        assert_eq!(
            close_messages[0].content.to_json(),
            phoenix_core::domain::db_schema::MessageContent::system(format!(
                "Close attempt {attempt} completed; this conversation is now archived in History."
            ))
            .to_json()
        );
        assert_eq!(
            close_row_counts(&fixture).await,
            vec![1, 1, 1, 1, 1, 2, 2, 2, 1],
            "one original run dispatches and physically retires exactly two resources"
        );
        let before_replay = durable_native_close_rows(&fixture, &run).await;
        let physical_before_replay = physical_snapshot(&git.root);
        // Simulate losing the first response: redeliver the original exact source
        // invocation, not a fresh tool id, retry run, fake delete or direct worker.
        let replay = tokio::time::timeout(Duration::from_secs(10), tool.run(input, context))
            .await
            .expect("exact-source admission replay must be bounded");
        assert!(replay.is_success(), "{}", replay.output());
        let replay: Value = serde_json::from_str(&replay.output()).unwrap();
        assert_eq!(replay["status"], "replay");
        assert_eq!(replay["request_id"], admitted["request_id"]);
        assert_eq!(replay["admitted"], admitted["admitted"]);
        assert_eq!(
            durable_native_close_rows(&fixture, &run).await,
            before_replay,
            "lost-response replay must not create another run, dispatch, outcome, or retirement effect"
        );
        assert_eq!(physical_snapshot(&git.root), physical_before_replay);
        assert_eq!(
            fixture
                .db
                .close_worktree_cleanup_plan(&attempt, &scope, &snapshot, &worktrees[0].resource)
                .await
                .unwrap(),
            Some(cleanup_plan)
        );
        assert_eq!(
            fixture
                .db
                .list_close_retirement_evidence(attempt.as_str())
                .await
                .unwrap(),
            outcomes
        );
        assert_eq!(
            git.run(&git.repo, &["worktree", "list", "--porcelain", "-z"]),
            registrations
        );
        assert_eq!(
            std::fs::symlink_metadata(&git.worktree).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(
            std::fs::symlink_metadata(&git.admin).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        git.assert_untargeted_state_preserved();
    }
    assert_eq!(physical_snapshot(&git.sibling), sibling_before_admission);
    assert_eq!(
        physical_snapshot(&git.sibling_admin),
        sibling_admin_before_admission
    );
    event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("source actor must shut down after qualification")
        .unwrap();
}

// Fixed schema queries: no dynamic column discovery or JSON encoding of blobs.
// Counts catch duplicate runs/effects; typed reads below compare their contents.
async fn close_row_counts(fixture: &CloseFixture) -> Vec<i64> {
    let mut counts = Vec::new();
    for query in [
        "SELECT COUNT(*) FROM close_obligations",
        "SELECT COUNT(*) FROM close_runs",
        "SELECT COUNT(*) FROM close_original_retirement_anchors",
        "SELECT COUNT(*) FROM close_attempt_scopes",
        "SELECT COUNT(*) FROM close_retirement_inventories",
        "SELECT COUNT(*) FROM close_expected_retirement_resources",
        "SELECT COUNT(*) FROM close_retirement_resource_dispatches",
        "SELECT COUNT(*) FROM close_retirement_resources",
        "SELECT COUNT(*) FROM native_close_initial_receipts",
    ] {
        counts.push(
            sqlx::query_scalar::<_, i64>(query)
                .fetch_one(fixture.db.pool())
                .await
                .unwrap(),
        );
    }
    counts
}

async fn durable_target_rows(fixture: &CloseFixture) -> Value {
    let target = fixture.db.get_conversation(TARGET_ID).await.unwrap();
    let scope: String = sqlx::query_scalar(
        "SELECT json_array(id, authority_kind, lifecycle, environment_kind, cwd,
                           worktree_path, branch_name, base_branch, worktree_id,
                           worktree_fingerprint, created_at, updated_at, retired_at,
                           retired_reason)
         FROM work_scopes
         WHERE id = (SELECT work_scope_id FROM conversations WHERE id = ?1)",
    )
    .bind(TARGET_ID)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    let metadata = fixture
        .db
        .read_ordinary_product_conversation_metadata(TARGET_ID)
        .await
        .unwrap();
    let messages = fixture.db.get_messages(TARGET_ID).await.unwrap();
    serde_json::json!({
        "target": target,
        "scope": scope,
        "lifecycle": metadata.lifecycle,
        "messages": messages,
    })
}

async fn durable_native_close_rows(fixture: &CloseFixture, run: &CloseRunRef) -> Value {
    let attempt = run.attempt_id.as_str();
    let obligation = fixture.db.get_close_obligation(attempt).await.unwrap();
    let persisted_run = fixture.db.get_close_run(run).await.unwrap();
    let scopes = fixture.db.list_close_attempt_scopes(attempt).await.unwrap();
    let expected = fixture
        .db
        .list_close_expected_retirement_resources(attempt)
        .await
        .unwrap();
    let evidence = fixture
        .db
        .list_close_retirement_evidence(attempt)
        .await
        .unwrap();
    let anchor: (String, String) = sqlx::query_as(
        "SELECT inspection_generation, inspection_fingerprint
         FROM close_original_retirement_anchors WHERE attempt_id = ?1",
    )
    .bind(attempt)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    // quote preserves identity values even if their storage class is BLOB.
    let dispatches: Vec<String> = sqlx::query_scalar(
        "SELECT json_array(attempt_id, scope, inspection_generation,
                           inspection_fingerprint, resource_kind, identity_kind,
                           identity_codec, quote(identity_value), dispatched_at_us)
         FROM close_retirement_resource_dispatches WHERE attempt_id = ?1
         ORDER BY scope, resource_kind, identity_kind, identity_value",
    )
    .bind(attempt)
    .fetch_all(fixture.db.pool())
    .await
    .unwrap();
    let accepted_at_us: i64 = sqlx::query_scalar(
        "SELECT accepted_at_us FROM native_close_initial_receipts WHERE attempt_id = ?1",
    )
    .bind(attempt)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    let run_times: (String, Option<String>) = sqlx::query_as(
        "SELECT CAST(started_at AS TEXT), CAST(completed_at AS TEXT)
         FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2",
    )
    .bind(attempt)
    .bind(run.ordinal.get())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    let process_successes: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT scope, resource_kind, identity_value, outcome, observed_at_us
         FROM close_process_step_successes WHERE attempt_id = ?1 AND run_ordinal = ?2
         ORDER BY scope, resource_kind, identity_value",
    )
    .bind(attempt)
    .bind(run.ordinal.get())
    .fetch_all(fixture.db.pool())
    .await
    .unwrap();
    let history: Vec<String> = sqlx::query_scalar(
        "SELECT json_array(history_id, attempt_id, scope, inspection_generation,
                           inspection_fingerprint, resource_kind, identity_kind,
                           identity_codec, quote(identity_value), proof_kind,
                           absence_basis, residual_reason, detail, recorded_at)
         FROM close_retirement_resource_history WHERE attempt_id = ?1 ORDER BY history_id",
    )
    .bind(attempt)
    .fetch_all(fixture.db.pool())
    .await
    .unwrap();
    serde_json::json!({
        "counts": close_row_counts(fixture).await,
        "accepted_at_us": accepted_at_us,
        "run_times": run_times,
        "process_successes": process_successes,
        "history": history,
        "obligation": obligation,
        "run": persisted_run,
        "scopes": scopes,
        "expected": expected,
        "evidence": evidence,
        "anchor": anchor,
        "dispatches": dispatches,
        "target": durable_target_rows(fixture).await,
    })
}
