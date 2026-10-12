//! Persisted member metadata projection with explicit live process observations.

use crate::db::{Database, DbResult};
use std::collections::HashMap;

/// Observed process facts required by full member hydration; no replay defaults.
pub(crate) struct MemberProcessObservations {
    pub shell: Option<String>,
    pub home_dir: String,
    pub terminal_uses_tmux: bool,
    pub browser_session_active: bool,
}

pub(crate) struct PersistedMemberObservations {
    seed_parent_slug: Option<String>,
    parent_conversation_slug: Option<String>,
    cached_pr: Option<crate::runtime::CachedPrSummary>,
}

impl PersistedMemberObservations {
    pub(crate) fn project(
        self,
        conv: &crate::db::Conversation,
        process: MemberProcessObservations,
    ) -> crate::runtime::EnrichedConversation {
        project_live_member(conv, self.metadata(conv), process)
    }
}

impl PersistedMemberObservations {
    fn metadata(
        self,
        conv: &crate::db::Conversation,
    ) -> super::persisted_member::PersistedMemberMetadata {
        let mut metadata = persisted_member_metadata(conv);
        metadata.seed_parent_slug = self.seed_parent_slug;
        metadata.parent_conversation_slug = self.parent_conversation_slug;
        metadata.cached_pr = self.cached_pr;
        metadata
    }

    pub(crate) fn project_persisted(
        self,
        conv: &crate::db::Conversation,
    ) -> super::persisted_member::PersistedMember {
        super::persisted_member::PersistedMember {
            id: conv.id.clone(),
            slug: conv.slug.clone(),
            title: conv.title.clone(),
            cwd: conv.cwd.clone(),
            parent_conversation_id: conv.parent_conversation_id.clone(),
            user_initiated: conv.user_initiated,
            state: crate::runtime::public_conversation_state(&conv.state),
            state_updated_at: conv.state_updated_at,
            created_at: conv.created_at,
            updated_at: conv.updated_at,
            archived: conv.archived,
            model: conv.model.clone(),
            effort: conv.effort,
            service_tier: conv.service_tier,
            project_id: conv.project_id.clone(),
            message_count: conv.message_count,
            transcript_generation: conv.transcript_generation,
            seed_parent_id: conv.seed_parent_id.clone(),
            seed_label: conv.seed_label.clone(),
            continued_in_conv_id: conv.continued_in_conv_id.clone(),
            chain_name: conv.chain_name.clone(),
            metadata: self.metadata(conv),
        }
    }
}

/// Bounded, read-only owner of persisted member enrichment.
pub(crate) struct MemberObservations {
    db: Database,
}

impl MemberObservations {
    pub(crate) async fn read(
        &self,
        conv: &crate::db::Conversation,
        include_cached_pr: bool,
    ) -> DbResult<PersistedMemberObservations> {
        let mut enriched = PersistedMemberObservations {
            seed_parent_slug: None,
            parent_conversation_slug: None,
            cached_pr: None,
        };
        if include_cached_pr {
            enriched.cached_pr = self.cached_pr_summary(conv).await?;
        }
        if let Some(parent_id) = conv.seed_parent_id.as_deref() {
            if let Ok(parent) = self.db.get_conversation(parent_id).await {
                enriched.seed_parent_slug = parent.slug;
            }
        }
        if let Some(parent_id) = conv.parent_conversation_id.as_deref() {
            if let Ok(parent) = self.db.get_conversation(parent_id).await {
                enriched.parent_conversation_slug = parent.slug;
            }
        }
        Ok(enriched)
    }
    pub(crate) async fn cached_pr_summary(
        &self,
        conv: &crate::db::Conversation,
    ) -> DbResult<Option<crate::runtime::CachedPrSummary>> {
        let Some(scope) = conv.attached_work_scope_id.as_ref() else {
            return Ok(None);
        };
        Ok(self
            .db
            .primary_work_scope_pr_association(scope)
            .await?
            .map(|pr| sidebar_cached_pr_summary(&pr)))
    }
    pub(crate) async fn cached_pr_summaries(
        &self,
        conversations: &[crate::db::Conversation],
    ) -> DbResult<HashMap<String, crate::runtime::CachedPrSummary>> {
        let scopes: Vec<_> = conversations
            .iter()
            .filter_map(|conv| conv.attached_work_scope_id.clone())
            .collect();
        let associations = self.db.primary_work_scope_pr_associations(&scopes).await?;
        Ok(associations
            .into_iter()
            .map(|(key, pr)| (key, sidebar_cached_pr_summary(&pr)))
            .collect())
    }
    pub(crate) fn new(db: Database) -> Self {
        Self { db }
    }
}

/// Preserves the list endpoint's unhydrated browser field, not an observation.
pub(super) fn project_list_member(
    conv: &crate::db::Conversation,
    shell: Option<String>,
    home_dir: String,
    terminal_uses_tmux: bool,
) -> crate::runtime::EnrichedConversation {
    project_live_member(
        conv,
        persisted_member_metadata(conv),
        MemberProcessObservations {
            shell,
            home_dir,
            terminal_uses_tmux,
            browser_session_active: false,
        },
    )
}

fn persisted_member_metadata(
    conv: &crate::db::Conversation,
) -> super::persisted_member::PersistedMemberMetadata {
    super::persisted_member::PersistedMemberMetadata {
        conv_mode_label: conv.conv_mode.label().to_string(),
        branch_name: conv.conv_mode.branch_name().map(String::from),
        worktree_path: conv
            .conv_mode
            .worktree_path()
            .filter(|s| !s.is_empty() && !s.starts_with("__LEGACY"))
            .map(String::from),
        base_branch: conv
            .conv_mode
            .base_branch()
            .filter(|s| !s.is_empty() && !s.starts_with("__LEGACY"))
            .map(String::from),
        task_title: conv.conv_mode.task_title().map(String::from),
        seed_parent_slug: None,
        parent_conversation_slug: None,
        work_scope_key: conversation_resource_scope(conv).stable_key(),
        cached_pr: None,
    }
}
fn project_live_member(
    conv: &crate::db::Conversation,
    metadata: super::persisted_member::PersistedMemberMetadata,
    process: MemberProcessObservations,
) -> crate::runtime::EnrichedConversation {
    let super::persisted_member::PersistedMemberMetadata {
        conv_mode_label,
        branch_name,
        worktree_path,
        base_branch,
        task_title,
        work_scope_key,
        seed_parent_slug,
        parent_conversation_slug,
        cached_pr,
    } = metadata;
    let MemberProcessObservations {
        shell,
        home_dir,
        terminal_uses_tmux,
        browser_session_active,
    } = process;
    crate::runtime::EnrichedConversation {
        conv_mode_label,
        branch_name,
        worktree_path,
        base_branch,
        task_title,
        work_scope_key,
        seed_parent_slug,
        parent_conversation_slug,
        cached_pr,
        shell,
        home_dir: Some(home_dir),
        terminal_uses_tmux,
        browser_session_active,
        creation_prompt: None,
        creation_error: None,
        inner: crate::runtime::PresentationConversation(conv.clone()),
    }
}

pub(super) fn conversation_resource_scope(
    conv: &crate::db::Conversation,
) -> crate::work_scope::ResourceScopeKey {
    match conv.attached_work_scope_id.clone() {
        Some(scope) => crate::work_scope::ResourceScopeKey::Work(scope),
        None if conv.runtime_role == crate::work_scope::RuntimeRole::Coordinator => {
            crate::work_scope::ResourceScopeKey::Coordinator
        }
        None if conv.runtime_role == crate::work_scope::RuntimeRole::SubAgent => {
            crate::work_scope::ResourceScopeKey::Unattached(conv.id.clone())
        }
        None => panic!("ordinary persisted conversation is missing its work scope"),
    }
}
fn sidebar_cached_pr_summary(
    pr: &crate::db::WorkScopePrAssociation,
) -> crate::runtime::CachedPrSummary {
    crate::runtime::CachedPrSummary {
        number: pr.pr_number,
        title: pr.title.clone(),
        url: pr.url.clone(),
        display_state: pr.display_state.clone(),
        base: pr.base.clone(),
        head: pr.head.clone(),
        feedback_status: pr.feedback_status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Copied pre-extraction constructor; synthetic normal facts replace ambient reads.
    fn legacy_projection(conv: &crate::db::Conversation) -> crate::runtime::EnrichedConversation {
        let process = normal_process();
        crate::runtime::EnrichedConversation {
            conv_mode_label: conv.conv_mode.label().to_string(),
            branch_name: conv.conv_mode.branch_name().map(String::from),
            worktree_path: conv
                .conv_mode
                .worktree_path()
                .filter(|s| !s.is_empty() && !s.starts_with("__LEGACY"))
                .map(String::from),
            base_branch: conv
                .conv_mode
                .base_branch()
                .filter(|s| !s.is_empty() && !s.starts_with("__LEGACY"))
                .map(String::from),
            task_title: conv.conv_mode.task_title().map(String::from),
            shell: process.shell,
            home_dir: Some(process.home_dir),
            seed_parent_slug: None,
            parent_conversation_slug: None,
            browser_session_active: process.browser_session_active,
            terminal_uses_tmux: process.terminal_uses_tmux,
            work_scope_key: conversation_resource_scope(conv).stable_key(),
            creation_prompt: None,
            creation_error: None,
            cached_pr: None,
            inner: crate::runtime::PresentationConversation(conv.clone()),
        }
    }
    async fn member(
        db: &Database,
        id: &str,
        parent: Option<&str>,
        seed: Option<&str>,
    ) -> crate::db::Conversation {
        db.create_conversation_with_project(
            id,
            &format!("slug-{id}"),
            "/tmp/member-observation",
            parent.is_none(),
            parent,
            None,
            None,
            &crate::db::ConvMode::Direct,
            None,
            seed,
            seed.map(|_| "Seed label"),
            crate::llm_language::LlmLanguage::default(),
        )
        .await
        .expect("create member")
    }

    #[tokio::test]
    async fn immutable_wire_projection_matches_pre_extraction_modes_and_states() {
        use crate::db::{ConvMode, NonEmptyString};
        use crate::state_machine::ConvState;

        let db = Database::open_in_memory().await.unwrap();
        let mut conv = member(&db, "projection", None, None).await;
        let service = MemberObservations::new(db.clone());
        let nonempty = |value: &str| NonEmptyString::new(value.to_owned()).unwrap();
        let modes = [
            ConvMode::Direct,
            ConvMode::Explore {
                worktree_path: Some(nonempty("__LEGACY_WORKTREE")),
                next_taskmd_id_hint: None,
            },
            ConvMode::Work {
                branch_name: nonempty("task-member"),
                worktree_path: nonempty("/tmp/member-worktree"),
                base_branch: nonempty("main"),
                task_id: nonempty("12345"),
                task_title: nonempty("Member task"),
            },
        ];
        let states = [
            ConvState::Idle,
            ConvState::LlmRequesting { attempt: 2 },
            ConvState::SeededLlmRequesting {
                seed_message_id: "seed-message".into(),
                attempt: 0,
            },
            ConvState::Terminal,
        ];
        for mode in modes {
            conv.conv_mode = mode;
            for state in &states {
                conv.state = state.clone();
                let before = serde_json::to_value(&conv).unwrap();
                let expected = serde_json::to_value(legacy_projection(&conv)).unwrap();
                let projected = service
                    .read(&conv, true)
                    .await
                    .unwrap()
                    .project(&conv, normal_process());
                let actual = serde_json::to_value(&projected).unwrap();
                assert_eq!(actual, expected);
                let persisted = serde_json::to_value(
                    service
                        .read(&conv, true)
                        .await
                        .unwrap()
                        .project_persisted(&conv),
                )
                .unwrap();
                for (key, value) in persisted.as_object().unwrap() {
                    assert_eq!(
                        actual.get(key).unwrap_or(&serde_json::Value::Null),
                        value,
                        "persisted public field {key}"
                    );
                }
                for field in [
                    "browser_session_active",
                    "terminal_uses_tmux",
                    "shell",
                    "home_dir",
                    "product_conversation_id",
                ] {
                    assert!(
                        persisted.get(field).is_none(),
                        "not persisted metadata: {field}"
                    );
                }
                assert_eq!(
                    actual["state"],
                    crate::runtime::public_conversation_state(state)
                );
                assert!(actual.get("product_conversation_id").is_none());
                assert_eq!(serde_json::to_value(&conv).unwrap(), before);
                assert_eq!(serde_json::to_value(&projected.inner.0).unwrap(), before);

                let mut list_expected = legacy_projection(&conv);
                list_expected.browser_session_active = false;
                let process = normal_process();
                assert_eq!(
                    serde_json::to_value(project_list_member(
                        &conv,
                        process.shell,
                        process.home_dir,
                        process.terminal_uses_tmux,
                    ))
                    .unwrap(),
                    serde_json::to_value(list_expected).unwrap(),
                );
            }
        }
        assert_eq!(
            db.get_conversation(&conv.id).await.unwrap().state,
            ConvState::Idle
        );
    }

    #[tokio::test]
    async fn process_facts_and_resource_scope_are_explicit() {
        let db = Database::open_in_memory().await.unwrap();
        let mut conv = member(&db, "scope", None, None).await;
        let service = MemberObservations::new(db);
        let scope = conv.attached_work_scope_id.as_ref().unwrap();
        assert_eq!(
            conversation_resource_scope(&conv).stable_key(),
            format!("work:{}", scope.as_str())
        );
        conv.attached_work_scope_id = None;
        for (role, expected_scope) in [
            (crate::work_scope::RuntimeRole::Coordinator, "coordinator"),
            (crate::work_scope::RuntimeRole::SubAgent, "unattached:scope"),
        ] {
            conv.runtime_role = role;
            let actual = serde_json::to_value(service.read(&conv, true).await.unwrap().project(
                &conv,
                MemberProcessObservations {
                    shell: None,
                    home_dir: "/different/home".into(),
                    terminal_uses_tmux: false,
                    browser_session_active: false,
                },
            ))
            .unwrap();
            assert_eq!(actual["work_scope_key"], expected_scope);
            assert!(actual.get("shell").is_none());
            assert_eq!(actual["home_dir"], "/different/home");
            assert_eq!(actual["terminal_uses_tmux"], false);
            assert_eq!(actual["browser_session_active"], false);
            assert!(actual.get("cached_pr").is_none());
        }
    }

    #[tokio::test]
    async fn parent_and_seed_slugs_resolve_independently_and_missing_parents_are_unlinked() {
        let db = Database::open_in_memory().await.unwrap();
        member(&db, "parent", None, None).await;
        member(&db, "seed", None, None).await;
        let service = MemberObservations::new(db.clone());
        let mut child = member(&db, "child", Some("parent"), Some("seed")).await;
        let before = serde_json::to_value(&child).unwrap();
        let actual = serde_json::to_value(
            service
                .read(&child, false)
                .await
                .unwrap()
                .project(&child, normal_process()),
        )
        .unwrap();
        assert_eq!(actual["parent_conversation_slug"], "slug-parent");
        assert_eq!(actual["seed_parent_slug"], "slug-seed");
        assert_eq!(
            serde_json::to_value(db.get_conversation("child").await.unwrap()).unwrap(),
            before
        );
        child.parent_conversation_id = Some("missing-parent".into());
        child.seed_parent_id = Some("missing-seed".into());
        for with_ids in [true, false] {
            if !with_ids {
                child.parent_conversation_id = None;
                child.seed_parent_id = None;
            }
            let actual = serde_json::to_value(
                service
                    .read(&child, false)
                    .await
                    .unwrap()
                    .project(&child, normal_process()),
            )
            .unwrap();
            assert!(actual.get("parent_conversation_slug").is_none());
            assert!(actual.get("seed_parent_slug").is_none());
        }
    }

    #[tokio::test]
    async fn cached_pr_single_batch_none_and_private_projection_use_persisted_associations() {
        let db = Database::open_in_memory().await.unwrap();
        let conv = member(&db, "cached", None, None).await;
        let no_pr = member(&db, "no-pr", None, None).await;
        let scope = conv.attached_work_scope_id.as_ref().unwrap();
        db.upsert_work_scope_pr_observations(
            scope,
            &[crate::db::WorkScopePrObservation {
                repo_owner: "example".into(),
                repo_name: "repo".into(),
                pr_number: 44,
                title: "Cached PR".into(),
                url: "https://github.com/example/repo/pull/44".into(),
                state: "OPEN".into(),
                draft: false,
                display_state: phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                base: "main".into(),
                head: "task-member".into(),
                github_updated_at: None,
            }],
        )
        .await
        .unwrap();
        db.update_work_scope_pr_feedback_status(
            scope,
            "example",
            "repo",
            44,
            phoenix_core::domain::pr_feedback_status::PrFeedbackStatus::InProgress,
        )
        .await
        .unwrap();
        let service = MemberObservations::new(db);
        let expected = serde_json::json!({
            "number": 44, "title": "Cached PR", "url": "https://github.com/example/repo/pull/44",
            "display_state": "open", "base": "main", "head": "task-member", "feedback_status": "in_progress",
        });
        assert_eq!(
            serde_json::to_value(service.cached_pr_summary(&conv).await.unwrap()).unwrap(),
            expected
        );
        assert!(service.cached_pr_summary(&no_pr).await.unwrap().is_none());
        let batch = service
            .cached_pr_summaries(&[conv.clone(), no_pr, conv.clone()])
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(
            serde_json::to_value(batch.values().next().unwrap()).unwrap(),
            expected
        );
        assert!(service.cached_pr_summaries(&[]).await.unwrap().is_empty());
        let included = serde_json::to_value(
            service
                .read(&conv, true)
                .await
                .unwrap()
                .project(&conv, normal_process()),
        )
        .unwrap();
        assert_eq!(included["cached_pr"], expected);
        let persisted = serde_json::to_value(
            service
                .read(&conv, true)
                .await
                .unwrap()
                .project_persisted(&conv),
        )
        .unwrap();
        assert_eq!(persisted["cached_pr"], expected);
        let excluded = serde_json::to_value(
            service
                .read(&conv, false)
                .await
                .unwrap()
                .project(&conv, normal_process()),
        )
        .unwrap();
        assert!(excluded.get("cached_pr").is_none());
    }

    fn normal_process() -> MemberProcessObservations {
        MemberProcessObservations {
            shell: Some("/bin/zsh".into()),
            home_dir: "/home/member".into(),
            terminal_uses_tmux: true,
            browser_session_active: true,
        }
    }
}
