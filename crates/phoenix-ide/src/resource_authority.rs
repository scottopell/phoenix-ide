use crate::db::{ConvMode, Conversation, Database, DbError};
use crate::work_scope::{
    EffectiveResourceAccess, ResourceAuthority, ResourceScopeKey, RuntimeRole,
};

pub(crate) struct ResolvedResourceAuthority {
    pub(crate) scope: ResourceScopeKey,
    pub(crate) authority: ResourceAuthority,
    pub(crate) actor: EffectiveResourceAccess,
}

pub(crate) async fn resolve_resource_authority(
    db: &Database,
    conversation: &Conversation,
) -> Result<ResolvedResourceAuthority, DbError> {
    let (scope, authority) = if let Some(work_scope_id) = &conversation.attached_work_scope_id {
        (
            ResourceScopeKey::Work(work_scope_id.clone()),
            db.get_conversation_work_scope_authority(&conversation.id)
                .await?
                .into(),
        )
    } else {
        let authority = match conversation.conv_mode {
            ConvMode::Explore { .. } => ResourceAuthority::Restricted,
            ConvMode::Direct
            | ConvMode::Work { .. }
            | ConvMode::Branch { .. }
            | ConvMode::DetachedApprovedTask { .. } => ResourceAuthority::Work,
        };
        (
            ResourceScopeKey::Unattached(conversation.id.clone()),
            authority,
        )
    };
    let actor = if authority == ResourceAuthority::Restricted
        && conversation.runtime_role == RuntimeRole::User
    {
        EffectiveResourceAccess::shared_restricted(conversation.id.clone())
    } else {
        EffectiveResourceAccess::new(conversation.id.clone(), authority)
    };
    Ok(ResolvedResourceAuthority {
        scope,
        authority,
        actor,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db::{ConvMode, Database};
    use phoenix_core::task_handoff::TaskApprovalHandoffData;

    pub(crate) fn approval() -> TaskApprovalHandoffData {
        TaskApprovalHandoffData {
            task_id: "12345".to_string(),
            task_title: "Approved objective".to_string(),
            title: "Approved objective".to_string(),
            priority: crate::task_source::Priority::P1,
            plan: "Execute it".to_string(),
            task_file: "tasks/12345-p1-in-progress--approved-objective.md".to_string(),
            artifact_body: "# Approved objective".to_string(),
        }
    }

    #[tokio::test]
    async fn unattached_subagent_authority_is_resolved_at_the_shared_boundary() {
        let db = Database::open_in_memory().await.unwrap();
        let parent = db
            .get_or_create_coordinator(
                Some("gpt-5.4"),
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        let child = db
            .create_subagent_conversation(
                "unattached-authority",
                "unattached-authority",
                "/tmp",
                &parent.id,
                "gpt-5.4",
                &ConvMode::Explore {
                    worktree_path: None,
                    next_taskmd_id_hint: None,
                },
                phoenix_core::llm_language::LlmLanguage::default(),
                None,
            )
            .await
            .unwrap();

        let resolved = resolve_resource_authority(&db, &child).await.unwrap();
        assert_eq!(
            resolved.scope,
            ResourceScopeKey::Unattached(child.id.clone())
        );
        assert_eq!(resolved.authority, ResourceAuthority::Restricted);
    }

    #[tokio::test]
    async fn persisted_scope_authority_promotes_explore_without_changing_mode() {
        let db = Database::open_in_memory().await.unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        db.create_conversation(
            &id,
            "approved-explore",
            "/tmp/approved-explore",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let before = db.get_conversation(id.as_str()).await.unwrap();
        let restricted = resolve_resource_authority(&db, &before).await.unwrap();
        assert_eq!(restricted.authority, ResourceAuthority::Restricted);

        db.persist_approved_task_authority(id.as_str(), &approval())
            .await
            .unwrap();
        let after = db.get_conversation(id.as_str()).await.unwrap();
        assert!(matches!(after.conv_mode, ConvMode::Explore { .. }));
        let promoted = resolve_resource_authority(&db, &after).await.unwrap();
        assert_eq!(promoted.authority, ResourceAuthority::Work);
    }
}
