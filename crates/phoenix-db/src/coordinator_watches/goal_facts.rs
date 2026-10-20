use phoenix_core::domain::watch_goal::{WatchGoalCapture, WatchGoalState, WatchGoalUsage};
use sqlx::{Row, Sqlite, Transaction};

use crate::conversation_goals::control::GoalControlInstallation;
use crate::{DbError, DbResult};

pub(super) fn marker(installation: GoalControlInstallation) -> &'static str {
    match installation {
        GoalControlInstallation::Disabled => "disabled",
        GoalControlInstallation::Installed => "captured",
    }
}

pub(super) async fn capture_transcript_tx(
    tx: &mut Transaction<'_, Sqlite>,
    installation: GoalControlInstallation,
    event_id: &str,
    transcript: &str,
) -> DbResult<()> {
    if installation == GoalControlInstallation::Disabled {
        return Ok(());
    }
    let owner: String =
        sqlx::query_scalar("SELECT product_conversation_id FROM conversations WHERE id=?")
            .bind(transcript)
            .fetch_one(&mut **tx)
            .await?;
    capture_inserted_tx(tx, installation, event_id, &owner).await
}

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

pub(super) fn decode(row: &sqlx::sqlite::SqliteRow) -> DbResult<Option<WatchGoalCapture>> {
    if row.try_get::<&str, _>("route_kind")? != "subscription" {
        return Ok(None);
    }
    let bad = || DbError::Serialization("invalid persisted watch goal capture".into());
    let capture = match row.try_get::<&str, _>("goal_capture_kind")? {
        "legacy" => WatchGoalCapture::LegacyNotCaptured,
        "disabled" => WatchGoalCapture::NotCapturedDisabled,
        "captured" => {
            let kind: &str = row.try_get("capture_kind")?;
            let owner_id: String = row.try_get("capture_owner_id")?;
            let version: i64 = row.try_get("capture_owner_version")?;
            let usage = match row.try_get::<&str, _>("capture_usage_kind")? {
                "unavailable_not_captured" => WatchGoalUsage::UnavailableNotCaptured,
                _ => return Err(bad()),
            };
            if owner_id.trim().is_empty() || version < 0 {
                return Err(bad());
            }
            match kind {
                "no_goal" => WatchGoalCapture::CapturedNoGoal {
                    owner_id,
                    owner_version: version,
                    usage,
                },
                "saved_goal" => {
                    let goal_id: String = row.try_get("capture_goal_id")?;
                    let objective: String = row.try_get("capture_objective")?;
                    let reason: Option<String> = row.try_get("capture_reason")?;
                    let state = match (row.try_get::<&str, _>("capture_state")?, reason) {
                        ("underway", None) => WatchGoalState::Underway,
                        ("blocked", Some(reason)) if !reason.trim().is_empty() => {
                            WatchGoalState::Blocked { reason }
                        }
                        ("complete", Some(reason)) if !reason.trim().is_empty() => {
                            WatchGoalState::Complete { reason }
                        }
                        ("exhausted", Some(reason)) if !reason.trim().is_empty() => {
                            WatchGoalState::Exhausted { reason }
                        }
                        ("suspended", Some(reason)) if !reason.trim().is_empty() => {
                            WatchGoalState::Suspended { reason }
                        }
                        _ => return Err(bad()),
                    };
                    if version == 0 || goal_id.trim().is_empty() || objective.trim().is_empty() {
                        return Err(bad());
                    }
                    WatchGoalCapture::CapturedSavedGoal {
                        owner_id,
                        goal_id,
                        objective,
                        owner_version: version,
                        state,
                        usage,
                    }
                }
                _ => return Err(bad()),
            }
        }
        _ => return Err(bad()),
    };
    Ok(Some(capture))
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
    let snapshot = &entry.snapshot;
    let version = snapshot.version.get();
    let watches: Vec<i64> = sqlx::query_scalar("SELECT w.id FROM coordinator_watches w
        JOIN product_conversations p ON p.id=w.source_product_conversation_id
        WHERE p.id=? AND w.ended_at_us IS NULL AND p.ordinary_lifecycle='open' AND w.enrolled_at_us<=?
          AND NOT EXISTS (SELECT 1 FROM close_obligations o WHERE o.product_conversation_id=p.id AND o.phase!='completed')")
        .bind(snapshot.owner.as_str()).bind(entry.recorded_at_us.get()).fetch_all(&mut **tx).await?;
    for watch_id in watches {
        let event_id = uuid::Uuid::new_v4().to_string();
        let inserted: Option<String> = sqlx::query_scalar("INSERT INTO coordinator_watch_events
            (event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us,goal_capture_kind)
            VALUES (?,?,'goal_change',?,?,?,'goal_changed',?,'captured')
            ON CONFLICT(source_occurrence_kind,source_occurrence_id,source_generation,watch_id) DO NOTHING RETURNING event_id")
            .bind(&event_id).bind(watch_id).bind(snapshot.owner.as_str()).bind(version).bind(transcript)
            .bind(entry.recorded_at_us.get()).fetch_optional(&mut **tx).await?;
        if let Some(id) = inserted {
            capture_inserted_tx(tx, installation, &id, snapshot.owner.as_str()).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
