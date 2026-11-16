use super::{
    source::{
        advance, candidates, capture, capture_range, chain_query, encoded, hash, hex, invalid,
        lane_name, member, member_page, owner, position, seq, topology,
    },
    PreparedScanBatch, ReservedScanAttempt, ScanBlocker, ScanContext, ScanCoveragePage,
    ScanCoverageState, ScanCoverageSummary, ScanFailure, ScanFreshness, ScanInput, ScanLane,
    ScanMemberCoverage, ScanMetrics, ScanPosition, ScanPreparation, ScanProfile, ScanReservation,
    SCAN_MAX_INPUT_BYTES, SCAN_RETRY_BACKOFF_US,
};
use crate::{coordinator_tracking, workflow::LocalAuthorityResult, Database, DbResult};
use sqlx::{Connection, Row, Sqlite, Transaction};

pub(super) async fn finish<T>(
    tx: Transaction<'_, Sqlite>,
    result: DbResult<T>,
) -> LocalAuthorityResult<DbResult<T>> {
    use LocalAuthorityResult::{
        DurableFactEstablished as Established, DurableFactUnclassified as Unclassified,
    };
    match result {
        Ok(value) => match tx.commit().await {
            Ok(()) => Established(Ok(value)),
            Err(_) => Unclassified,
        },
        Err(error) => match tx.rollback().await {
            Ok(()) => Established(Err(error)),
            Err(_) => Unclassified,
        },
    }
}
macro_rules! write_tx {
    ($self:ident,$tx:ident,$body:expr) => {{
        use LocalAuthorityResult::DurableFactEstablished as Established;
        let mut connection = match $self.pool.acquire().await {
            Ok(c) => c,
            Err(e) => return Established(Err(e.into())),
        };
        let mut $tx = match connection.begin_with("BEGIN IMMEDIATE").await {
            Ok(t) => t,
            Err(e) => return Established(Err(e.into())),
        };
        let result = $body.await;
        finish($tx, result).await
    }};
}
pub(super) use write_tx;

impl Database {
    /// Prepares only a bounded own-Coordinator prose window. No provider call or wakeup.
    /// # Errors
    /// Storage failures preserve the local authority classification.
    pub async fn prepare_coordinator_scan(
        &self,
        profile: Option<&ScanProfile>,
        now_us: i64,
    ) -> LocalAuthorityResult<DbResult<ScanPreparation>> {
        write_tx!(self, tx, prepare(&mut tx, profile, now_us))
    }
    /// Charges a durable attempt against the exact consumed batch before dispatch.
    /// # Errors
    /// Storage failures preserve the local authority classification.
    pub async fn reserve_coordinator_scan(
        &self,
        batch: Box<PreparedScanBatch>,
        input_bytes: usize,
        now_us: i64,
    ) -> LocalAuthorityResult<DbResult<ScanReservation>> {
        write_tx!(self, tx, reserve(&mut tx, batch, input_bytes, now_us))
    }
    /// Records a known failed inference without advancing source coverage.
    /// # Errors
    /// Storage failures preserve the local authority classification.
    pub async fn fail_coordinator_scan(
        &self,
        attempt: &ReservedScanAttempt,
        failure: ScanFailure,
        metrics: ScanMetrics,
        now_us: i64,
    ) -> LocalAuthorityResult<DbResult<()>> {
        write_tx!(self, tx, async {
            let Some((owner, _)) = owner(&mut tx).await? else {
                return Err(crate::DbError::CurrentCoordinatorRequired);
            };
            if owner != attempt.batch.owner_product_id {
                return Err(crate::DbError::CurrentCoordinatorRequired);
            }
            validate_attempt(&mut tx, attempt).await?;
            let (kind, detail) = match failure {
                ScanFailure::Provider(s) => ("provider", s),
                ScanFailure::Timeout => ("timeout", "inference exceeded its timeout".into()),
                ScanFailure::InvalidOutput(s) => ("invalid_output", s),
                ScanFailure::Storage(s) => ("storage", s),
            };
            complete_attempt(&mut tx, attempt, "failed", Some((kind, &detail)), &metrics).await?;
            set_blocker(&mut tx, ScanBlocker::InferenceFailed, None, now_us).await?;
            Ok(())
        })
    }
    /// Reads a bounded coverage page. Coverage never means human review or permission.
    /// # Errors
    /// Requires the current singleton Coordinator and valid pagination.
    pub async fn read_coordinator_scan_coverage(
        &self,
        source: &str,
        after_member: Option<i64>,
    ) -> DbResult<ScanCoveragePage> {
        let mut tx = self.pool.begin().await?;
        coordinator_tracking::require_owner(&mut tx, source).await?;
        let result = coverage(&mut tx, after_member).await?;
        tx.commit().await?;
        Ok(result)
    }
}
pub(super) async fn set_blocker(
    tx: &mut Transaction<'_, Sqlite>,
    reason: ScanBlocker,
    next: Option<i64>,
    now: i64,
) -> DbResult<()> {
    let reason = serde_json::to_value(reason).map_err(|e| invalid(e.to_string()))?;
    sqlx::query("UPDATE coordinator_scan_state SET last_blocker=?1,next_resume_at_us=?2,observed_at_us=?3 WHERE singleton=1").bind(reason.as_str()).bind(next).bind(now).execute(&mut **tx).await?;
    Ok(())
}
#[allow(clippy::too_many_lines)]
async fn prepare(
    tx: &mut Transaction<'_, Sqlite>,
    profile: Option<&ScanProfile>,
    now: i64,
) -> DbResult<ScanPreparation> {
    if now < 0 {
        return Err(invalid("negative scan time"));
    }
    let Some(topology) = topology(tx).await? else {
        return Ok(ScanPreparation::Idle);
    };
    let owner = &topology.owner;
    coordinator_tracking::ensure_registry(tx, owner).await?;
    sqlx::query("INSERT INTO coordinator_scan_state(singleton,activation_member_ordinal,discovery_next_ordinal,observed_at_us) VALUES(1,?1,0,?2) ON CONFLICT(singleton) DO NOTHING").bind(topology.maximum).bind(now).execute(&mut **tx).await?;
    let state = sqlx::query("SELECT * FROM coordinator_scan_state WHERE singleton=1")
        .fetch_one(&mut **tx)
        .await?;
    let activated: i64 = state.try_get("activation_member_ordinal")?;
    let next: i64 = state.try_get("discovery_next_ordinal")?;
    let mut selected = member_page(tx, &topology, next).await?;
    if selected.is_empty() {
        selected = member_page(tx, &topology, 0).await?;
    }
    let revisit = selected.last().map_or(0, |last| {
        if last.ordinal >= topology.maximum {
            0
        } else {
            last.ordinal + 1
        }
    });
    if !selected.iter().any(|m| m.id == topology.head) {
        selected.push(
            member(tx, &topology, &topology.head)
                .await?
                .ok_or_else(|| invalid("coordinator head missing from own topology"))?,
        );
    }
    for member in &selected {
        let boundary = if member.ordinal > activated {
            0
        } else {
            member.tail
        };
        sqlx::query("INSERT INTO coordinator_scan_members(transcript_id,continuation_ordinal,generation,activation_boundary,frontier_revision,live_seq,historical_seq) VALUES(?1,?2,?3,?4,0,?4,?4) ON CONFLICT(transcript_id) DO NOTHING").bind(&member.id).bind(member.ordinal).bind(member.generation).bind(boundary).execute(&mut **tx).await?;
        sqlx::query("UPDATE coordinator_scan_members SET generation=?2,activation_boundary=?3,frontier_revision=frontier_revision+1,live_seq=?3,historical_seq=?3,live_message_id=NULL,live_part=NULL,live_offset=NULL,live_digest=NULL,historical_message_id=NULL,historical_part=NULL,historical_offset=NULL,historical_digest=NULL WHERE transcript_id=?1 AND generation<>?2").bind(&member.id).bind(member.generation).bind(member.tail).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE coordinator_scan_state SET discovery_next_ordinal=?1,observed_at_us=?2 WHERE singleton=1").bind(revisit).bind(now).execute(&mut **tx).await?;
    let streak: i64 = state.try_get("live_streak")?;
    let preferred = if streak >= 3 {
        [ScanLane::Historical, ScanLane::Live]
    } else {
        [ScanLane::Live, ScanLane::Historical]
    };
    let mut waiting = None;
    for lane in preferred {
        let rows=sqlx::query(chain_query!(" SELECT s.*,m.transcript_generation,m.tail FROM coordinator_scan_members s JOIN members m ON m.id=s.transcript_id WHERE s.generation=m.transcript_generation AND ((?3='live' AND (s.live_message_id IS NOT NULL OR s.live_seq<m.tail)) OR (?3='historical' AND s.historical_seq>0)) ORDER BY s.selected_at_us,s.continuation_ordinal DESC LIMIT 32"))
            .bind(&topology.head).bind(&topology.owner).bind(lane_name(lane)).fetch_all(&mut **tx).await?;
        for row in &rows {
            let transcript: String = row.try_get("transcript_id")?;
            let generation: i64 = row.try_get("generation")?;
            if generation != row.try_get::<i64, _>("transcript_generation")? {
                continue;
            }
            let start = position(row, lane)?;
            let pending = match lane {
                ScanLane::Live => {
                    matches!(start, ScanPosition::Fragment { .. })
                        || seq(&start) < row.try_get::<i64, _>("tail")?
                }
                ScanLane::Historical => seq(&start) > 0,
            };
            if !pending {
                continue;
            }
            sqlx::query(
                "UPDATE coordinator_scan_members SET selected_at_us=?2 WHERE transcript_id=?1",
            )
            .bind(&transcript)
            .bind(now)
            .execute(&mut **tx)
            .await?;
            let captured = match capture(tx, &transcript, generation, lane, &start).await {
                Ok(c) => c,
                Err(crate::DbError::Serialization(error)) => {
                    tracing::warn!(%transcript,%error,"scanner source remains pending");
                    waiting = Some((ScanBlocker::SourceUnavailable, None));
                    continue;
                }
                Err(error) => return Err(error),
            };
            if captured.fragments.is_empty() {
                advance(tx, &transcript, lane, &captured.end).await?;
                return Ok(ScanPreparation::Progress);
            }
            let Some(profile) = profile else {
                set_blocker(tx, ScanBlocker::RouteUnavailable, None, now).await?;
                return Ok(ScanPreparation::Wait {
                    reason: ScanBlocker::RouteUnavailable,
                    next_at_us: None,
                });
            };
            let (candidates, omitted_active_candidates, omitted_retired_candidates) =
                candidates(tx, &captured.fragments).await?;
            let revision: i64 = sqlx::query_scalar(
                "SELECT revision FROM coordinator_tracking_registry WHERE singleton=1",
            )
            .fetch_one(&mut **tx)
            .await?;
            let frontier_revision = row.try_get("frontier_revision")?;
            let binding_id = hash(encoded(&(
                owner,
                &transcript,
                generation,
                lane,
                &start,
                &captured.end,
                frontier_revision,
                &captured.source_digest,
                revision,
                profile.connection,
                &profile.endpoint_hash,
                &profile.prompt_hash,
            ))?);
            let resume: Option<i64> = sqlx::query_scalar(
                "SELECT resumes_at_us FROM coordinator_scan_work WHERE binding_id=?1",
            )
            .bind(&binding_id)
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
            if resume.is_some_and(|t| t > now) {
                waiting = Some((ScanBlocker::RetryBackoff, resume));
                continue;
            }
            if let Some(reason) = budget_pause(tx, lane, now, 0).await? {
                waiting = Some(reason);
                continue;
            }
            let input = ScanInput {
                scan_context: ScanContext {
                    lane,
                    new_capture_freshness: ScanFreshness::Unverified,
                    captured_at_us: now,
                    transcript_id: transcript,
                    source_generation: generation,
                    registry_revision: revision,
                    omitted_active_candidates,
                    omitted_retired_candidates,
                },
                fragments: captured.fragments,
                candidates,
            };
            if profile
                .instruction_bytes
                .saturating_add(encoded(&input)?.len())
                > SCAN_MAX_INPUT_BYTES
            {
                return Err(invalid("scanner composed request exceeds input budget"));
            }
            return Ok(ScanPreparation::Ready(Box::new(PreparedScanBatch {
                input,
                binding_id,
                profile: profile.clone(),
                start,
                end: captured.end,
                frontier_revision,
                source_digest: captured.source_digest,
                owner_product_id: owner.clone(),
            })));
        }
    }
    if let Some((reason, next_at_us)) = waiting {
        set_blocker(tx, reason, next_at_us, now).await?;
        return Ok(ScanPreparation::Wait { reason, next_at_us });
    }
    sqlx::query("UPDATE coordinator_scan_state SET last_blocker=NULL,next_resume_at_us=NULL WHERE singleton=1").execute(&mut **tx).await?;
    Ok(ScanPreparation::Idle)
}
async fn budget_pause(
    tx: &mut Transaction<'_, Sqlite>,
    lane: ScanLane,
    now: i64,
    bytes: i64,
) -> DbResult<Option<(ScanBlocker, Option<i64>)>> {
    const HOUR: i64 = 3_600_000_000;
    const DAY: i64 = 86_400_000_000;
    let day = now / DAY * DAY;
    let hour = now / HOUR * HOUR;
    let row=sqlx::query("SELECT COUNT(*) AS calls,COALESCE(SUM(a.input_bytes),0) AS bytes,COALESCE(SUM(a.reserved_at_us>=?2),0) AS hourly,COALESCE(SUM(w.lane='historical'),0) AS historical,COALESCE(SUM(w.lane='historical' AND a.reserved_at_us>=?2),0) AS historical_hourly FROM coordinator_scan_attempts a JOIN coordinator_scan_work w ON w.binding_id=a.binding_id WHERE a.reserved_at_us>=?1").bind(day).bind(hour).fetch_one(&mut **tx).await?;
    let paused = if row.try_get::<i64, _>("calls")? >= 128
        || row.try_get::<i64, _>("bytes")?.saturating_add(bytes) > 3 * 1024 * 1024
    {
        Some((ScanBlocker::GlobalBudgetPaused, Some(day + DAY)))
    } else if row.try_get::<i64, _>("hourly")? >= 16 {
        Some((ScanBlocker::GlobalBudgetPaused, Some(hour + HOUR)))
    } else if lane == ScanLane::Historical && row.try_get::<i64, _>("historical")? >= 64 {
        Some((ScanBlocker::HistoricalBudgetPaused, Some(day + DAY)))
    } else if lane == ScanLane::Historical && row.try_get::<i64, _>("historical_hourly")? >= 4 {
        Some((ScanBlocker::HistoricalBudgetPaused, Some(hour + HOUR)))
    } else {
        None
    };
    Ok(paused)
}
pub(super) async fn batch_current(
    tx: &mut Transaction<'_, Sqlite>,
    batch: &PreparedScanBatch,
) -> DbResult<bool> {
    let Some(topology) = topology(tx).await? else {
        return Ok(false);
    };
    if topology.owner != batch.owner_product_id
        || member(tx, &topology, &batch.input.scan_context.transcript_id)
            .await?
            .is_none_or(|m| m.generation != batch.input.scan_context.source_generation)
    {
        return Ok(false);
    }
    let row = sqlx::query("SELECT * FROM coordinator_scan_members WHERE transcript_id=?1")
        .bind(&batch.input.scan_context.transcript_id)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(row) = row else { return Ok(false) };
    let revision: i64 =
        sqlx::query_scalar("SELECT revision FROM coordinator_tracking_registry WHERE singleton=1")
            .fetch_one(&mut **tx)
            .await?;
    if row.try_get::<i64, _>("frontier_revision")? != batch.frontier_revision
        || revision != batch.input.scan_context.registry_revision
        || position(&row, batch.input.scan_context.lane)? != batch.start
    {
        return Ok(false);
    }
    let fresh = capture_range(
        tx,
        &batch.input.scan_context.transcript_id,
        batch.input.scan_context.source_generation,
        batch.input.scan_context.lane,
        &batch.start,
        Some(&batch.end),
    )
    .await?;
    Ok(fresh.source_digest == batch.source_digest
        && fresh.end == batch.end
        && encoded(&fresh.fragments)? == encoded(&batch.input.fragments)?)
}
async fn reserve(
    tx: &mut Transaction<'_, Sqlite>,
    batch: Box<PreparedScanBatch>,
    input_bytes: usize,
    now: i64,
) -> DbResult<ScanReservation> {
    if now < 0
        || input_bytes == 0
        || input_bytes > SCAN_MAX_INPUT_BYTES
        || input_bytes
            < batch
                .profile
                .instruction_bytes
                .saturating_add(encoded(&batch.input)?.len())
    {
        return Err(invalid("invalid final composed scanner input budget"));
    }
    if !batch_current(tx, &batch).await? {
        return Ok(ScanReservation::Conflict);
    }
    if let Some((reason, next_at_us)) = budget_pause(
        tx,
        batch.input.scan_context.lane,
        now,
        i64::try_from(input_bytes).map_err(|e| invalid(e.to_string()))?,
    )
    .await?
    {
        set_blocker(tx, reason, next_at_us, now).await?;
        return Ok(ScanReservation::Paused { reason, next_at_us });
    }
    sqlx::query("INSERT INTO coordinator_scan_work(binding_id,transcript_id,source_generation,lane,frontier_revision,registry_revision,source_digest,connection,endpoint_hash,prompt_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(binding_id) DO NOTHING").bind(&batch.binding_id).bind(&batch.input.scan_context.transcript_id).bind(batch.input.scan_context.source_generation).bind(lane_name(batch.input.scan_context.lane)).bind(batch.frontier_revision).bind(batch.input.scan_context.registry_revision).bind(&batch.source_digest).bind(batch.profile.connection.as_str()).bind(hex(batch.profile.endpoint_hash)).bind(hex(batch.profile.prompt_hash)).execute(&mut **tx).await?;
    let row =
        sqlx::query("SELECT cycle,resumes_at_us FROM coordinator_scan_work WHERE binding_id=?1")
            .bind(&batch.binding_id)
            .fetch_one(&mut **tx)
            .await?;
    let mut cycle: i64 = row.try_get("cycle")?;
    let resume: Option<i64> = row.try_get("resumes_at_us")?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_scan_attempts WHERE binding_id=?1 AND cycle=?2",
    )
    .bind(&batch.binding_id)
    .bind(cycle)
    .fetch_one(&mut **tx)
    .await?;
    let ordinal = if count >= 2 {
        if resume.is_some_and(|t| t > now) {
            set_blocker(tx, ScanBlocker::RetryBackoff, resume, now).await?;
            return Ok(ScanReservation::Paused {
                reason: ScanBlocker::RetryBackoff,
                next_at_us: resume,
            });
        }
        cycle += 1;
        sqlx::query(
            "UPDATE coordinator_scan_work SET cycle=?2,resumes_at_us=NULL WHERE binding_id=?1",
        )
        .bind(&batch.binding_id)
        .bind(cycle)
        .execute(&mut **tx)
        .await?;
        1
    } else {
        count + 1
    };
    let id=sqlx::query("INSERT INTO coordinator_scan_attempts(binding_id,cycle,ordinal,reserved_at_us,input_bytes,state) VALUES(?1,?2,?3,?4,?5,'reserved')").bind(&batch.binding_id).bind(cycle).bind(ordinal).bind(now).bind(i64::try_from(input_bytes).map_err(|e|invalid(e.to_string()))?).execute(&mut **tx).await?.last_insert_rowid();
    if ordinal == 2 {
        sqlx::query("UPDATE coordinator_scan_work SET resumes_at_us=?2 WHERE binding_id=?1")
            .bind(&batch.binding_id)
            .bind(now.saturating_add(SCAN_RETRY_BACKOFF_US))
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("UPDATE coordinator_scan_state SET live_streak=CASE WHEN ?1='historical' THEN 0 ELSE MIN(live_streak+1,3) END,last_blocker=NULL,next_resume_at_us=NULL,observed_at_us=?2 WHERE singleton=1").bind(lane_name(batch.input.scan_context.lane)).bind(now).execute(&mut **tx).await?;
    Ok(ScanReservation::Reserved(ReservedScanAttempt { id, batch }))
}
pub(super) async fn validate_attempt(
    tx: &mut Transaction<'_, Sqlite>,
    attempt: &ReservedScanAttempt,
) -> DbResult<String> {
    sqlx::query_scalar("SELECT state FROM coordinator_scan_attempts WHERE id=?1 AND binding_id=?2")
        .bind(attempt.id)
        .bind(&attempt.batch.binding_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| invalid("scanner attempt does not own this binding"))
}
pub(super) async fn complete_attempt(
    tx: &mut Transaction<'_, Sqlite>,
    attempt: &ReservedScanAttempt,
    state: &str,
    failure: Option<(&str, &str)>,
    metrics: &ScanMetrics,
) -> DbResult<()> {
    let detail = failure.map(|(_, s)| {
        let mut end = s.len().min(1024);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.get(..end)
            .expect("validated UTF8 failure-detail boundary")
            .to_owned()
    });
    let usage = metrics.usage.as_ref();
    let n = |v: u64| i64::try_from(v).map_err(|e| invalid(e.to_string()));
    sqlx::query("UPDATE coordinator_scan_attempts SET state=?2,failure_kind=?3,failure_detail=?4,elapsed_ms=?5,input_tokens=?6,cache_read_tokens=?7,cache_creation_tokens=?8,output_tokens=?9 WHERE id=?1 AND state='reserved'").bind(attempt.id).bind(state).bind(failure.map(|(s,_)|s)).bind(detail).bind(n(metrics.elapsed_ms)?).bind(usage.and_then(|u|u.input_tokens).map(n).transpose()?).bind(usage.and_then(|u|u.cache_read_tokens).map(n).transpose()?).bind(usage.and_then(|u|u.cache_creation_tokens).map(n).transpose()?).bind(usage.and_then(|u|u.output_tokens).map(n).transpose()?).execute(&mut **tx).await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn coverage(
    tx: &mut Transaction<'_, Sqlite>,
    after: Option<i64>,
) -> DbResult<ScanCoveragePage> {
    if after.is_some_and(|n| n < 0) {
        return Err(invalid("negative member cursor"));
    }
    let Some(topology) = topology(tx).await? else {
        return Err(crate::DbError::CurrentCoordinatorRequired);
    };
    let state = sqlx::query("SELECT * FROM coordinator_scan_state WHERE singleton=1")
        .fetch_optional(&mut **tx)
        .await?;
    let totals=sqlx::query(chain_query!(" SELECT COUNT(*) AS total,COUNT(s.transcript_id) AS enrolled,
        COALESCE(SUM(s.transcript_id IS NULL OR s.generation<>m.transcript_generation OR s.live_message_id IS NOT NULL OR s.live_seq<m.tail),0) AS live,
        COALESCE(SUM(s.transcript_id IS NULL OR s.generation<>m.transcript_generation OR s.historical_seq>0),0) AS historical,
        COALESCE(SUM(s.transcript_id IS NOT NULL AND s.generation<>m.transcript_generation),0) AS stale
        FROM members m LEFT JOIN coordinator_scan_members s ON s.transcript_id=m.id"))
        .bind(&topology.head).bind(&topology.owner).fetch_one(&mut **tx).await?;
    let count = |column: &str| -> DbResult<usize> {
        usize::try_from(totals.try_get::<i64, _>(column)?).map_err(|e| invalid(e.to_string()))
    };
    let total = count("total")?;
    let mut summary = ScanCoverageSummary {
        enrolled_members: count("enrolled")?,
        pending_live_members: count("live")?,
        pending_historical_members: count("historical")?,
        generation_stale_members: count("stale")?,
        blocker: None,
        next_resume_at_us: None,
    };
    let mut rows=sqlx::query(chain_query!(" SELECT m.*,s.* FROM members m LEFT JOIN coordinator_scan_members s ON s.transcript_id=m.id WHERE m.ordinal>?3 ORDER BY m.ordinal LIMIT 9"))
        .bind(&topology.head).bind(&topology.owner).bind(after.unwrap_or(-1)).fetch_all(&mut **tx).await?;
    let has_more = rows.len() > 8;
    rows.truncate(8);
    let captured_at_us = if let Some(state) = state {
        summary.blocker = state
            .try_get::<Option<String>, _>("last_blocker")?
            .map(|v| {
                serde_json::from_value(serde_json::Value::String(v))
                    .map_err(|e| invalid(e.to_string()))
            })
            .transpose()?;
        summary.next_resume_at_us = state.try_get("next_resume_at_us")?;
        state.try_get("observed_at_us")?
    } else {
        0
    };
    let mut members = Vec::with_capacity(rows.len());
    for row in rows {
        let generation: Option<i64> = row.try_get("generation")?;
        let source_generation: i64 = row.try_get("transcript_generation")?;
        let (
            covered_generation,
            live_boundary,
            historical_boundary,
            live_frontier,
            historical_frontier,
            status,
        ) = if let Some(generation) = generation {
            let live = position(&row, ScanLane::Live)?;
            let historical = position(&row, ScanLane::Historical)?;
            let generation_changed = generation != source_generation;
            let live_pending = generation_changed
                || matches!(live, ScanPosition::Fragment { .. })
                || seq(&live) < row.try_get::<i64, _>("tail")?;
            let historical_pending = generation_changed || seq(&historical) > 0;
            (
                Some(generation),
                row.try_get("activation_boundary")?,
                Some(row.try_get("activation_boundary")?),
                Some(live),
                Some(historical),
                if generation_changed {
                    ScanCoverageState::GenerationStale
                } else if live_pending || historical_pending {
                    ScanCoverageState::Incomplete
                } else {
                    ScanCoverageState::Covered
                },
            )
        } else {
            (None, 0, None, None, None, ScanCoverageState::PendingInitial)
        };
        members.push(ScanMemberCoverage {
            transcript_id: row.try_get("id")?,
            continuation_ordinal: row.try_get("ordinal")?,
            source_generation,
            covered_generation,
            live_boundary,
            historical_boundary,
            live_frontier,
            historical_frontier,
            state: status,
        });
    }
    let next_member = if has_more {
        members.last().map(|m| m.continuation_ordinal)
    } else {
        None
    };
    let omitted_members = total.saturating_sub(members.len());
    Ok(ScanCoveragePage {
        captured_at_us,
        summary,
        members,
        has_more,
        next_member,
        omitted_members,
    })
}
