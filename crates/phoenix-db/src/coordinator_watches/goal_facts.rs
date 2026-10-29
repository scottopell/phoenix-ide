use crate::conversation_goals::control::GoalControlInstallation;
use crate::DbResult;
use sqlx::{Row, Sqlite, Transaction};

pub(super) async fn capture_inserted_tx(
    tx: &mut Transaction<'_, Sqlite>,
    installation: GoalControlInstallation,
    event_id: &str,
    owner: &str,
) -> DbResult<()> {
    if installation == GoalControlInstallation::Disabled {
        return Ok(());
    }
    let current = sqlx::query("SELECT h.version,h.goal_id,g.objective,h.state,h.reason
        FROM conversation_goal_history h JOIN conversation_goal_ids g ON g.goal_id=h.goal_id AND g.owner_id=h.owner_id
        WHERE h.owner_id=? ORDER BY h.version DESC LIMIT 1")
        .bind(owner).fetch_optional(&mut **tx).await?;
    let (kind, version, goal_id, objective, state, reason) = match current {
        Some(row) => (
            "saved_goal",
            row.try_get::<i64, _>("version")?,
            Some(row.try_get::<String, _>("goal_id")?),
            Some(row.try_get::<String, _>("objective")?),
            Some(row.try_get::<String, _>("state")?),
            row.try_get::<Option<String>, _>("reason")?,
        ),
        None => ("no_goal", 0, None, None, None, None),
    };
    sqlx::query(
        "INSERT INTO coordinator_watch_goal_captures
        (event_id,kind,owner_id,owner_version,goal_id,objective,state,reason,usage_kind)
        VALUES (?,?,?,?,?,?,?,?,'unavailable_not_captured')",
    )
    .bind(event_id)
    .bind(kind)
    .bind(owner)
    .bind(version)
    .bind(goal_id)
    .bind(objective)
    .bind(state)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn settled_change_tx(
    tx: &mut Transaction<'_, Sqlite>,
    installation: GoalControlInstallation,
    entry: &phoenix_core::domain::conversation_goal::GoalHistoryEntry,
    transcript: &str,
) -> DbResult<()> {
    if installation == GoalControlInstallation::Disabled {
        return Ok(());
    }
    let snapshot = entry.snapshot();
    let version = snapshot.version().get();
    let watches: Vec<i64> = sqlx::query_scalar("SELECT w.id FROM coordinator_watches w
        JOIN product_conversations p ON p.id=w.source_product_conversation_id
        WHERE p.id=? AND w.ended_at_us IS NULL AND p.ordinary_lifecycle='open' AND w.enrolled_at_us<=?
          AND NOT EXISTS (SELECT 1 FROM close_obligations o WHERE o.product_conversation_id=p.id AND o.phase!='completed')")
        .bind(snapshot.owner().as_str()).bind(entry.recorded_at_us().get()).fetch_all(&mut **tx).await?;
    for watch_id in watches {
        let event_id = uuid::Uuid::new_v4().to_string();
        let inserted: Option<String> = sqlx::query_scalar("INSERT INTO coordinator_watch_events
            (event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us,goal_capture_kind)
            VALUES (?,?,'goal_change',?,?,?,'goal_changed',?,'captured')
            ON CONFLICT(source_occurrence_kind,source_occurrence_id,source_generation,watch_id) DO NOTHING RETURNING event_id")
            .bind(&event_id).bind(watch_id).bind(snapshot.owner().as_str()).bind(version).bind(transcript)
            .bind(entry.recorded_at_us().get()).fetch_optional(&mut **tx).await?;
        if let Some(id) = inserted {
            capture_inserted_tx(tx, installation, &id, snapshot.owner().as_str()).await?;
        }
    }
    Ok(())
}
