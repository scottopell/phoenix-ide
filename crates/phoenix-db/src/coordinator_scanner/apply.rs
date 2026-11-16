use super::{source::*, storage::*, *};
use crate::{
    coordinator_tracking::{self, TrackingEvidence, TrackingInitialReview, TrackingRecord},
    workflow::LocalAuthorityResult,
    Database, DbResult,
};
use sqlx::{Connection, Sqlite, Transaction};
use std::collections::HashSet;

impl Database {
    /// Applies only exact-source additions and source coverage under one CAS transaction.
    /// # Errors
    /// Storage failures preserve local authority; definitive conflicts/rejections finalize metrics.
    pub async fn apply_coordinator_scan(
        &self,
        attempt: &ReservedScanAttempt,
        result: &ScanResult,
        metrics: ScanMetrics,
        now_us: i64,
    ) -> LocalAuthorityResult<DbResult<ScanApplyOutcome>> {
        write_tx!(self, tx, apply(&mut tx, attempt, result, &metrics, now_us))
    }
}
struct EvidenceSource<'a> {
    fragment: &'a ScanFragment,
    start: usize,
    end: usize,
    evidence: TrackingEvidence,
}
fn quotes<'a>(
    batch: &'a PreparedScanBatch,
    quotes: &[ScanQuote],
    instance: &str,
) -> Result<Vec<EvidenceSource<'a>>, ScanRejection> {
    if quotes.is_empty() || quotes.len() > 16 {
        return Err(ScanRejection::InvalidQuote {
            detail: "one to16 exact source quotes required".into(),
        });
    }
    let mut out = Vec::new();
    for quote in quotes {
        let fragment = batch
            .input
            .fragments
            .iter()
            .find(|f| f.fragment_id == quote.fragment_id)
            .ok_or_else(|| ScanRejection::InvalidQuote {
                detail: "quote references a fragment outside the captured batch".into(),
            })?;
        if quote.quotation.trim().is_empty() {
            return Err(ScanRejection::InvalidQuote {
                detail: "empty source quotation".into(),
            });
        }
        let matches = fragment
            .text
            .match_indices(&quote.quotation)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(ScanRejection::InvalidQuote {
                detail: "source quotation must identify exactly one captured UTF8 span".into(),
            });
        }
        let start = fragment.utf8_start + matches[0].0;
        let end = start + quote.quotation.len();
        let evidence = TrackingEvidence {
            instance_id: instance.to_owned(),
            transcript_id: batch.input.scan_context.transcript_id.clone(),
            message_id: fragment.message_id.clone(),
            quotation: quote.quotation.clone(),
        };
        if encoded(&evidence)
            .map_err(|e| ScanRejection::InvalidQuote {
                detail: e.to_string(),
            })?
            .len()
            > 4096
        {
            return Err(ScanRejection::InvalidQuote {
                detail: "complete source evidence entry exceeds4KiB".into(),
            });
        }
        out.push(EvidenceSource {
            fragment,
            start,
            end,
            evidence,
        });
    }
    Ok(out)
}
fn primary(fragment: &ScanFragment) -> bool {
    fragment.role == ScanRole::User
        && matches!(
            fragment.origin,
            InputOrigin::UserApi
                | InputOrigin::UnknownHistorical
                | InputOrigin::SlackGateway { .. }
                | InputOrigin::RemoteApi { .. }
        )
}
async fn apply(
    tx: &mut Transaction<'_, Sqlite>,
    attempt: &ReservedScanAttempt,
    result: &ScanResult,
    metrics: &ScanMetrics,
    now: i64,
) -> DbResult<ScanApplyOutcome> {
    let state = validate_attempt(tx, attempt).await?;
    let result_digest = hash(encoded(result)?);
    let receipt: Option<String> = sqlx::query_scalar(
        "SELECT result_digest FROM coordinator_scan_applications WHERE binding_id=?1",
    )
    .bind(&attempt.batch.binding_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(receipt) = receipt {
        if receipt == result_digest {
            complete_attempt(tx, attempt, "applied", None, metrics).await?;
            return Ok(ScanApplyOutcome::Replayed);
        }
        complete_attempt(tx, attempt, "conflict", None, metrics).await?;
        return Ok(ScanApplyOutcome::Conflict);
    }
    if state != "reserved" {
        return Ok(ScanApplyOutcome::Conflict);
    }
    if !batch_current(tx, &attempt.batch).await? {
        complete_attempt(tx, attempt, "conflict", None, metrics).await?;
        return Ok(ScanApplyOutcome::Conflict);
    }
    let instance: String =
        sqlx::query_scalar("SELECT instance_id FROM instance_identity WHERE singleton_key=1")
            .fetch_one(&mut **tx)
            .await?;
    let ScanResult::Complete { actions } = result;
    let validate = || -> Result<Vec<Vec<EvidenceSource<'_>>>, ScanRejection> {
        if encoded(result)
            .map_err(|e| ScanRejection::InvalidOutput {
                detail: e.to_string(),
            })?
            .len()
            > SCAN_MAX_OUTPUT_BYTES
            || metrics
                .usage
                .as_ref()
                .and_then(|u| u.output_tokens)
                .is_none_or(|n| n > u64::from(SCAN_MAX_OUTPUT_TOKENS))
        {
            return Err(ScanRejection::InvalidOutput{detail:"accepted result requires at most16KiB encoded output and reported output tokens at most2048".into()});
        }
        if actions.len() > 16 {
            return Err(ScanRejection::InvalidOutput {
                detail: "at most16 scanner actions".into(),
            });
        }
        let mut checked = Vec::new();
        for action in actions {
            let source_quotes = match action {
                ScanAction::New { quotes, .. } | ScanAction::Append { quotes, .. } => quotes,
            };
            let evidence = quotes(&attempt.batch, source_quotes, &instance)?;
            match action {
                ScanAction::New {
                    kind,
                    title,
                    note,
                    resumption,
                    ..
                } => {
                    if !evidence.iter().any(|e| primary(e.fragment)) {
                        return Err(ScanRejection::InvalidCapture{detail:"new capture requires primary apparent user intent; assistant/event/quoted context alone is insufficient".into()});
                    }
                    coordinator_tracking::validate_scanner_record(*kind, title, note, resumption)
                        .map_err(|e| ScanRejection::InvalidCapture {
                        detail: e.to_string(),
                    })?;
                }
                ScanAction::Append { item_id, .. } => {
                    if !attempt
                        .batch
                        .input
                        .candidates
                        .iter()
                        .any(|c| c.id == *item_id)
                    {
                        return Err(ScanRejection::InvalidCapture {
                            detail: "append target is not an identified captured candidate".into(),
                        });
                    }
                }
            }
            checked.push(evidence);
        }
        Ok(checked)
    };
    let checked = match validate() {
        Ok(v) => v,
        Err(reason) => {
            complete_attempt(
                tx,
                attempt,
                "rejected",
                Some(("invalid_output", &reason.to_string())),
                metrics,
            )
            .await?;
            set_blocker(tx, ScanBlocker::InvalidOutput, None, now).await?;
            return Ok(ScanApplyOutcome::Rejected(reason));
        }
    };
    let mut created = Vec::new();
    let mut changed = HashSet::new();
    for (action, sources) in actions.iter().zip(&checked) {
        let evidence = sources
            .iter()
            .map(|s| s.evidence.clone())
            .collect::<Vec<_>>();
        let id = match action {
            ScanAction::New {
                kind,
                title,
                note,
                resumption,
                ..
            } => {
                let record = TrackingRecord {
                    kind: *kind,
                    title: title.clone(),
                    note: note.clone(),
                    resumption: resumption.clone(),
                    references: Vec::new(),
                    evidence,
                };
                let id = coordinator_tracking::create_tracking_item_tx(
                    tx,
                    &record,
                    TrackingInitialReview::NeedsReview,
                    now,
                )
                .await?;
                sqlx::query(
                    "INSERT INTO coordinator_scan_item_origins(item_id,attempt_id) VALUES(?1,?2)",
                )
                .bind(&id)
                .bind(attempt.id)
                .execute(&mut **tx)
                .await?;
                changed.insert(id.clone());
                created.push(id.clone());
                id
            }
            ScanAction::Append { item_id, .. } => {
                if coordinator_tracking::append_evidence(tx, item_id, &evidence, now).await? {
                    changed.insert(item_id.clone());
                }
                item_id.clone()
            }
        };
        for source in sources {
            let ordinal:i64=sqlx::query_scalar("SELECT ordinal FROM coordinator_tracking_evidence WHERE item_id=?1 AND instance_id=?2 AND transcript_id=?3 AND message_id=?4 AND digest=?5").bind(&id).bind(&source.evidence.instance_id).bind(&source.evidence.transcript_id).bind(&source.evidence.message_id).bind(hash(source.evidence.quotation.as_bytes())).fetch_one(&mut **tx).await?;
            let origin = serde_json::to_value(&source.fragment.origin)
                .map_err(|e| invalid(e.to_string()))?;
            let origin = origin
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("origin lacks its canonical discriminator"))?;
            sqlx::query("INSERT INTO coordinator_scan_evidence_origins(item_id,evidence_ordinal,origin_kind,source_role,source_generation,fragment_id,utf8_start,utf8_end,attempt_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(item_id,evidence_ordinal) DO NOTHING").bind(&id).bind(ordinal).bind(origin).bind(if source.fragment.role==ScanRole::User{"user"}else{"assistant"}).bind(attempt.batch.input.scan_context.source_generation).bind(&source.fragment.fragment_id).bind(i64::try_from(source.start).map_err(|e|invalid(e.to_string()))?).bind(i64::try_from(source.end).map_err(|e|invalid(e.to_string()))?).bind(attempt.id).execute(&mut **tx).await?;
        }
    }
    for id in &changed {
        if !created.contains(id) {
            sqlx::query("UPDATE coordinator_tracking_items SET revision=revision+1,updated_at_us=?2 WHERE id=?1").bind(id).bind(now).execute(&mut **tx).await?;
        }
    }
    if !changed.is_empty() {
        sqlx::query(
            "UPDATE coordinator_tracking_registry SET revision=revision+1 WHERE singleton=1",
        )
        .execute(&mut **tx)
        .await?;
    }
    let revision: i64 =
        sqlx::query_scalar("SELECT revision FROM coordinator_tracking_registry WHERE singleton=1")
            .fetch_one(&mut **tx)
            .await?;
    advance(
        tx,
        &attempt.batch.input.scan_context.transcript_id,
        attempt.batch.input.scan_context.lane,
        &attempt.batch.end,
    )
    .await?;
    sqlx::query("INSERT INTO coordinator_scan_applications(binding_id,attempt_id,result_digest,registry_revision,applied_at_us) VALUES(?1,?2,?3,?4,?5)").bind(&attempt.batch.binding_id).bind(attempt.id).bind(result_digest).bind(revision).bind(now).execute(&mut **tx).await?;
    complete_attempt(tx, attempt, "applied", None, metrics).await?;
    sqlx::query("UPDATE coordinator_scan_state SET last_blocker=NULL,next_resume_at_us=NULL,observed_at_us=?1 WHERE singleton=1").bind(now).execute(&mut **tx).await?;
    Ok(ScanApplyOutcome::Applied {
        created_item_ids: created,
        changed_item_count: changed.len(),
        registry_revision: revision,
    })
}
