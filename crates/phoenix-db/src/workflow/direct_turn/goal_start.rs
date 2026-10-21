use super::{
    conflict, insert_direct_turn_workflow_tx, next_direct_turn_id_tx, to_i64, to_u64,
    AcceptAuthoritativeTurn, AcceptedDisposition, ClientTurnKey, ConversationAuthority, DbError,
    DbResult, GoalStartReceiptId, PreparedTurn, Timestamp, TurnConflict,
};
use phoenix_core::domain::conversation_goal::{GoalExecution, GoalStartInput};

mod adoption;
mod context;
pub use adoption::{GoalStartAdoptionInput, GoalStartAdoptionOutcome};
pub use context::GoalStartRequestContext;

pub(crate) async fn insert_goal_start_turn_tx(
    tx: &mut super::super::WorkflowTx<'_>,
    input: &GoalStartInput,
    transcript: &str,
    accepted_at: Timestamp,
    controls: crate::conversation_goals::control::GoalControlInstallation,
) -> DbResult<()> {
    let turn_id = next_direct_turn_id_tx(tx).await?;
    let workflow_id = super::super::next_global_workflow_id_tx(tx).await?;
    let phoenix_core::domain::conversation_goal::GoalStartRequester::Owner { principal_id } =
        &input.requester
    else {
        return Err(DbError::Serialization(
            "Start requester is not owner".into(),
        ));
    };
    let start_id = sqlx::query("INSERT INTO conversation_goal_starts (owner_id,request_id,goal_id,observed_version,requester_kind,owner_principal_id,historical_workflow_id,historical_turn_id,historical_transcript_id,live_turn_id) VALUES (?,?,?,?,'owner',?,?,?,?,?)")
        .bind(input.goal().owner.as_str()).bind(input.request_id.to_string())
        .bind(input.goal().goal_id.as_str()).bind(input.goal().version.get())
        .bind(principal_id.to_string()).bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(to_i64(turn_id.0, "turn_id")?).bind(transcript)
        .bind(to_i64(turn_id.0, "turn_id")?).execute(&mut *tx.tx).await?.last_insert_rowid();
    let receipt = GoalStartReceiptId::new(to_u64(start_id, "Start receipt")?)
        .ok_or_else(|| conflict(TurnConflict::CorruptAggregate("Start identity")))?;
    let prepared =
        PreparedTurn::from_goal_start(&ConversationAuthority(transcript.to_owned()), receipt);
    let client_key = format!("goal-start:{}", receipt.get());
    insert_direct_turn_workflow_tx(
        tx,
        workflow_id,
        turn_id,
        &AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new(client_key.clone())
                .ok_or_else(|| conflict(TurnConflict::CorruptAggregate("Start client key")))?,
            prepared: prepared.clone(),
            disposition: AcceptedDisposition::Runtime,
            accepted_at,
        },
    )
    .await?;
    sqlx::query("INSERT INTO durable_turns (turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,canonical_message_id,owns_conversation,created_at,updated_at,origin_kind,input_kind,materialized,goal_start_id) VALUES (?,?,?,?,?,NULL,'Runtime',0,NULL,1,?,?,NULL,'goal_start',0,?)")
        .bind(to_i64(turn_id.0, "turn_id")?)
        .bind(to_i64(workflow_id.0, "workflow_id")?)
        .bind(transcript).bind(client_key).bind(prepared.fingerprint())
        .bind(to_i64(accepted_at.0, "created_at")?)
        .bind(to_i64(accepted_at.0, "updated_at")?)
        .bind(to_i64(receipt.get(), "goal_start_id")?)
        .execute(&mut *tx.tx).await?;
    let admitted = GoalExecution::parse(
        to_i64(workflow_id.0, "workflow_id")?,
        to_i64(turn_id.0, "turn_id")?,
    )
    .map_err(|_| conflict(TurnConflict::CorruptAggregate("Start execution identity")))?;
    crate::conversation_goals::capture_goal_at_new_turn_tx(&mut tx.tx, admitted).await?;
    crate::conversation_goals::control::capture_runtime_control_tx(&mut tx.tx, admitted, controls)
        .await
        .map_err(crate::conversation_goals::control::db_error)?;
    Ok(())
}
