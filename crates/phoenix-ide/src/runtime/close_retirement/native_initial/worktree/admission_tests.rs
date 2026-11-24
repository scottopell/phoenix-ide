//! Hosted-only real admission coverage; every repository and database is disposable.
use super::*;
use crate::runtime::executor::SourceEntryController;
use phoenix_core::{
    domain::{
        close::{CapturedWorktreeIdentity, CloseOwnedResourceInventory},
        db_schema::SourceToolCall,
        llm_types::ContentBlock,
        sm_state::{AssistantMessage, ToolCall, ToolInput},
    },
    llm_language::LlmLanguage,
};
use phoenix_db::{
    native_close::{NativeCloseInitialResult, NativeCloseIntent},
    CaptureCloseRetirementInventoryRequest, CaptureCloseRetirementInventoryScopeRequest,
    RecordCloseRetirementDispatchRequest, ReplaceCloseInspectionRequest,
    ReplaceCloseInspectionScopeRequest,
};
use std::{collections::BTreeSet, time::Duration};

fn executing_state(source: &SourceToolCall, intent: &NativeCloseIntent) -> ConvState {
    let input = intent.source_input();
    ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            &source.tool_use_id,
            ToolInput::Unknown {
                name: "close_conversation".into(),
                input: input.clone(),
            },
        ),
        remaining_tools: Vec::new(),
        completed_results: Vec::new(),
        pending_sub_agents: Vec::new(),
        assistant_message: AssistantMessage::new(
            source.message_id.clone(),
            vec![ContentBlock::ToolUse {
                id: source.tool_use_id.clone(),
                name: "close_conversation".into(),
                input,
            }],
            None,
            None,
        ),
    }
}

struct Fixture {
    _files: tempfile::TempDir,
    db: Database,
    controller: SourceEntryController,
    first: WorktreeGitObservation,
}

fn fixture_git(home: &Path, args: &[&str]) {
    let mut command = phoenix_core::git::command();
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    let output = super::super::super::wait_bounded_child_output(
        child,
        Instant::now() + Duration::from_secs(60),
        "isolated Worktree fixture",
    )
    .unwrap();
    assert!(
        output.status.success(),
        "Git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Fixture {
    async fn new(budget: Duration) -> Self {
        let files = tempfile::tempdir().unwrap();
        let repository = files.path().join("repository");
        let linked = files.path().join("linked");
        fixture_git(
            files.path(),
            &[
                "init",
                "--quiet",
                "--initial-branch=main",
                repository.to_str().unwrap(),
            ],
        );
        fixture_git(
            files.path(),
            &[
                "-C",
                repository.to_str().unwrap(),
                "-c",
                "user.name=Close fixture",
                "-c",
                "user.email=close@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "base",
            ],
        );
        fixture_git(
            files.path(),
            &[
                "-C",
                repository.to_str().unwrap(),
                "-c",
                "core.hooksPath=/dev/null",
                "worktree",
                "add",
                "--quiet",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let fingerprint = observe_linked_worktree_fingerprint(&linked).unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let target = db
            .create_conversation(
                "original-worktree-target",
                "original Worktree",
                linked.to_str().unwrap(),
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let scope = target.attached_work_scope_id.unwrap();
        sqlx::query(
            "UPDATE work_scopes SET environment_kind = 'allocated_worktree', cwd = ?1,
             worktree_path = ?1, worktree_id = 'original-worktree', worktree_fingerprint = ?2,
             branch_name = 'fixture', base_branch = 'main' WHERE id = ?3",
        )
        .bind(linked.to_str().unwrap())
        .bind(fingerprint)
        .bind(scope.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        let source_call = SourceToolCall {
            message_id: "original-worktree-message".into(),
            tool_use_id: "original-worktree-call".into(),
        };
        let intent = NativeCloseIntent::Initial {
            target: target.product_conversation_id,
        };
        let live_source = executing_state(&source_call, &intent);
        db.update_conversation_state(&source.id, &live_source)
            .await
            .unwrap();
        let command = NativeCloseCommand::new(source.id, source_call, intent).unwrap();
        let NativeCloseInitialResult::NewlyAdmitted(receipt) = db
            .apply_native_close_initial(&command)
            .await
            .established()
            .unwrap()
            .unwrap()
        else {
            panic!("expected original admission")
        };
        db.prepare_native_close_initial_execution(&command, &receipt, &live_source)
            .await
            .established()
            .unwrap()
            .unwrap();
        assert_eq!(
            db.try_settle_native_close_initial(&command, &receipt, &live_source)
                .await
                .established()
                .unwrap()
                .unwrap(),
            phoenix_db::native_close::NativeCloseInitialSettlement::Advanced(
                phoenix_db::native_close::NativeCloseInitialExecution {
                    run: receipt.admitted.clone(),
                    phase: phoenix_core::domain::close::ClosePhase::AwaitingRetirementInspection,
                },
            ),
        );
        db.replace_native_close_initial_inspection(
            &command,
            &receipt,
            &live_source,
            ReplaceCloseInspectionRequest {
                attempt_id: receipt.admitted.attempt_id.clone(),
                scopes: vec![ReplaceCloseInspectionScopeRequest {
                    scope: scope.clone(),
                    snapshot: CloseRetirementSnapshot::parse(
                        "original-generation",
                        "original-fingerprint",
                    )
                    .unwrap(),
                    losses: vec![],
                }],
            },
            None,
        )
        .await
        .established()
        .unwrap()
        .unwrap();
        let snapshot = db
            .get_close_obligation(receipt.admitted.attempt_id.as_str())
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone();
        let captured = db
            .list_close_attempt_scopes(receipt.admitted.attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(captured.len(), 1);
        let Some(CapturedWorktreeIdentity::Resolved(identity)) =
            captured[0].captured_worktree.clone()
        else {
            panic!("original linked Worktree must be resolved")
        };
        let expected = db
            .capture_native_close_initial_retirement_inventory(
                &command,
                &receipt,
                &live_source,
                CaptureCloseRetirementInventoryRequest {
                    attempt_id: receipt.admitted.attempt_id.clone(),
                    snapshot,
                    scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                        scope,
                        inventory: CloseOwnedResourceInventory {
                            worktree: Some(identity.clone()),
                            work_scopes: BTreeSet::new(),
                            bash_process_groups: BTreeSet::new(),
                            tmux_servers: BTreeSet::new(),
                            pty_sessions: BTreeSet::new(),
                            browser_sessions: BTreeSet::new(),
                            equivalent_live_resources: BTreeSet::new(),
                        },
                    }],
                },
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        let resource = expected
            .into_iter()
            .find(|entry| entry.resource.kind() == RetiredResourceKind::Worktree)
            .unwrap();
        db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &live_source,
            RecordCloseRetirementDispatchRequest {
                attempt_id: receipt.admitted.attempt_id.clone(),
                scope: resource.scope.clone(),
                snapshot: resource.snapshot.clone(),
                resource: resource.resource.clone(),
            },
        )
        .await
        .established()
        .unwrap()
        .unwrap();
        // The one deadline starts after fixture setup and is never refreshed.
        let deadline = Instant::now() + budget;
        let prepared = prepare_original_worktree_git_until(&identity, deadline)
            .unwrap()
            .unwrap();
        let mut controller = SourceEntryController::new(command.clone(), deadline.into());
        let _ = controller.revalidate(&live_source, &command);
        let first = prepared.spawn_status(
            tokio::runtime::Handle::current(),
            db.clone(),
            command,
            receipt,
            live_source,
            resource.scope,
            resource.snapshot,
            resource.resource,
            controller.worker(),
            FatalLocalAuthorityFence::new(),
        );
        Self {
            _files: files,
            db,
            controller,
            first,
        }
    }

    async fn completed_first(&mut self) -> NativeWorktreeGitContinuation {
        let output = self
            .first
            .observe()
            .await
            .expect("genuine original admission must run Git status");
        assert!(output.status.success());
        self.first
            .take_for_continuation()
            .expect("completed worker transfers original context")
    }
}

#[tokio::test]
async fn second_status_reuses_original_authority_and_rechecks_after_wait() {
    // A positive second operation prevents all negative cases from passing
    // merely because the fixture never established real admission.
    let mut fixture = Fixture::new(Duration::from_secs(60)).await;
    let continuation = fixture.completed_first().await;
    let request = continuation.original_effect.receipt.request_id;
    let scope = continuation.original_effect.scope.clone();
    let snapshot = continuation.original_effect.snapshot.clone();
    let deadline = continuation.original_effect.deadline;
    let second = continuation.spawn_status(tokio::runtime::Handle::current());
    second
        .observe()
        .await
        .expect("unchanged original authority permits second status");
    let same = second.take_for_continuation().unwrap();
    assert_eq!(same.original_effect.receipt.request_id, request);
    assert_eq!(same.original_effect.scope, scope);
    assert_eq!(same.original_effect.snapshot, snapshot);
    assert_eq!(same.original_effect.deadline, deadline);
    assert!(!same.original_effect.leaf_release_pending_classification);
    drop(same);
}

#[derive(Clone, Copy)]
enum ReentryRefusal {
    ControllerRevoked,
    ReceiptChanged,
    DurableSourceLost,
    OriginalDeadlineExpired,
}

async fn refuses_second_callback(reason: ReentryRefusal) {
    let budget = if matches!(reason, ReentryRefusal::OriginalDeadlineExpired) {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(60)
    };
    let mut fixture = Fixture::new(budget).await;
    let mut continuation = fixture.completed_first().await;
    match reason {
        ReentryRefusal::ControllerRevoked => {
            let _ = fixture.controller.revoke();
        }
        ReentryRefusal::ReceiptChanged => {
            continuation.original_effect.receipt.request_id = uuid::Uuid::new_v4();
        }
        ReentryRefusal::DurableSourceLost => {
            fixture
                .db
                .update_conversation_state(
                    continuation.original_effect.command.source_transcript_id(),
                    &ConvState::Idle,
                )
                .await
                .unwrap();
        }
        ReentryRefusal::OriginalDeadlineExpired => {
            tokio::time::sleep_until(continuation.original_effect.deadline.into()).await;
        }
    }
    let runtime = tokio::runtime::Handle::current();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_calls = Arc::clone(&calls);
    let (entry, continuation) = tokio::task::spawn_blocking(move || {
        let entry = continuation.original_effect.admitted_leaf(&runtime, || {
            // This is the same synchronous boundary that contains Git spawn.
            // No negative test creates a child if this assertion later fails.
            worker_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        let confirmed = match &entry {
            Ok((_, NativeCloseInitialEntryRelease::Confirmed))
            | Err(NativeCloseInitialFirstEffectError::NotStarted(_))
            | Err(NativeCloseInitialFirstEffectError::Rejected {
                release: NativeCloseInitialEntryRelease::Confirmed,
                ..
            }) => true,
            _ => false,
        };
        assert!(
            confirmed,
            "isolated negative fixture must release its database writer"
        );
        continuation.original_effect.classify_confirmed_release();
        (entry, continuation)
    })
    .await
    .unwrap();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a second operation cannot reuse prior admission after authority loss"
    );
    match reason {
        ReentryRefusal::ControllerRevoked | ReentryRefusal::OriginalDeadlineExpired => {
            assert!(matches!(
                entry,
                Ok((
                    WorkerEntryResult::Refused,
                    NativeCloseInitialEntryRelease::Confirmed
                ))
            ));
        }
        ReentryRefusal::ReceiptChanged | ReentryRefusal::DurableSourceLost => {
            assert!(matches!(
                entry,
                Err(NativeCloseInitialFirstEffectError::Rejected {
                    release: NativeCloseInitialEntryRelease::Confirmed,
                    ..
                })
            ));
        }
    }
    assert!(
        !continuation
            .original_effect
            .leaf_release_pending_classification
    );
    // Also exercise the production supervisor with that same refused context;
    // the direct callback check above proves the spawn boundary stays closed.
    let second = continuation.spawn_status(tokio::runtime::Handle::current());
    assert!(second.observe().await.is_err());
    assert!(second.take_for_continuation().is_none());
}

#[tokio::test]
async fn second_callback_rechecks_live_controller_after_status_wait() {
    refuses_second_callback(ReentryRefusal::ControllerRevoked).await;
}

#[tokio::test]
async fn second_callback_rechecks_original_receipt_after_status_wait() {
    refuses_second_callback(ReentryRefusal::ReceiptChanged).await;
}

#[tokio::test]
async fn second_callback_rechecks_durable_source_after_status_wait() {
    refuses_second_callback(ReentryRefusal::DurableSourceLost).await;
}

#[tokio::test]
async fn second_callback_cannot_refresh_original_deadline_after_status_wait() {
    refuses_second_callback(ReentryRefusal::OriginalDeadlineExpired).await;
}

#[tokio::test]
async fn second_status_rechecks_registration_after_status_wait() {
    let mut fixture = Fixture::new(Duration::from_secs(60)).await;
    let continuation = fixture.completed_first().await;
    let marker = continuation.owner.prepared.inspection_path.join(".git");
    let bytes = std::fs::read(&marker).unwrap();
    let replacement = marker.with_file_name(".git.replacement");
    std::fs::write(&replacement, bytes).unwrap();
    std::fs::rename(&replacement, &marker).unwrap();
    let second = continuation.spawn_status(tokio::runtime::Handle::current());
    let error = second
        .observe()
        .await
        .expect_err("replacement registration cannot authorize Git");
    assert!(error.contains("binding changed"), "{error}");
    assert!(second.take_for_continuation().is_none());
}

#[tokio::test]
async fn mutation_executor_rechecks_same_original_source_for_every_owned_effect() {
    use super::super::super::WorktreeMutationExecutor as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut fixture = Fixture::new(Duration::from_secs(60)).await;
    let mut continuation = fixture.completed_first().await;
    let mut controller = fixture.controller;
    let runtime = tokio::runtime::Handle::current();
    let calls = Arc::new(AtomicUsize::new(0));
    let worker_calls = Arc::clone(&calls);
    tokio::task::spawn_blocking(move || {
        let original_deadline = continuation.original_effect.deadline;
        let original_receipt = continuation.original_effect.receipt.request_id;
        let mut executor = super::mutation::OriginalWorktreeMutationExecutor::new(
            &mut continuation.original_effect,
            &runtime,
        );
        let first = Arc::clone(&worker_calls);
        executor
            .execute(move || {
                first.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        assert_eq!(worker_calls.load(Ordering::SeqCst), 1);
        let _ = controller.revoke();
        let second = Arc::clone(&worker_calls);
        let refusal = executor
            .execute(move || {
                second.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err();
        let third = Arc::clone(&worker_calls);
        assert_eq!(
            executor
                .execute(move || {
                    third.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .unwrap_err(),
            refusal
        );
        drop(executor);
        assert_eq!(worker_calls.load(Ordering::SeqCst), 1);
        assert_eq!(continuation.original_effect.deadline, original_deadline);
        assert_eq!(
            continuation.original_effect.receipt.request_id,
            original_receipt
        );
        assert!(
            !continuation
                .original_effect
                .leaf_release_pending_classification
        );
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mutation_executor_classifies_syscall_failure_before_separately_admitted_compensation() {
    use super::super::super::WorktreeMutationExecutor as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut fixture = Fixture::new(Duration::from_secs(60)).await;
    let mut continuation = fixture.completed_first().await;
    let runtime = tokio::runtime::Handle::current();
    let calls = Arc::new(AtomicUsize::new(0));
    let worker_calls = Arc::clone(&calls);
    tokio::task::spawn_blocking(move || {
        let original_deadline = continuation.original_effect.deadline;
        let mut executor = super::mutation::OriginalWorktreeMutationExecutor::new(
            &mut continuation.original_effect,
            &runtime,
        );
        let first = Arc::clone(&worker_calls);
        let result = executor.execute(move || {
            first.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>("confirmed syscall failure".to_string())
        });
        assert_eq!(result.unwrap_err(), "confirmed syscall failure");
        let second = Arc::clone(&worker_calls);
        executor
            .execute(move || {
                second.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        drop(executor);
        assert_eq!(continuation.original_effect.deadline, original_deadline);
        assert!(
            !continuation
                .original_effect
                .leaf_release_pending_classification
        );
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
