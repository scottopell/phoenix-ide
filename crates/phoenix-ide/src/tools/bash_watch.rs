use std::collections::HashMap;
use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::work_scope::WorkScope;

use super::{Tool, ToolContext, ToolOutput};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BashWatchTrigger {
    CommandExited { handle: String },
    OutputContains { handle: String, text: String },
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BashWatchWakeIntent {
    ResumeAgent { instruction: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BashWatchContract {
    pub trigger: BashWatchTrigger,
    pub wake: BashWatchWakeIntent,
}

#[derive(Debug, Clone, Serialize)]
pub struct BashWatch {
    pub watch_id: String,
    pub work_scope: WorkScope,
    pub creating_conversation_id: String,
    pub contract: BashWatchContract,
    pub created_at_ms: u128,
}

#[derive(Debug, Default)]
pub struct BashWatchRegistry {
    inner: RwLock<HashMap<String, BashWatch>>,
}

impl BashWatchRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(
        &self,
        work_scope: WorkScope,
        creating_conversation_id: String,
        contract: BashWatchContract,
    ) -> BashWatch {
        let watch = BashWatch {
            watch_id: format!("bw-{}", uuid::Uuid::new_v4()),
            work_scope,
            creating_conversation_id,
            contract,
            created_at_ms: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        };
        self.inner
            .write()
            .await
            .insert(watch.watch_id.clone(), watch.clone());
        watch
    }

    pub async fn remove(&self, watch_id: &str) -> Option<BashWatch> {
        self.inner.write().await.remove(watch_id)
    }

    pub async fn list_for_scope(&self, work_scope: &WorkScope) -> Vec<BashWatch> {
        let mut watches: Vec<_> = self
            .inner
            .read()
            .await
            .values()
            .filter(|watch| &watch.work_scope == work_scope)
            .cloned()
            .collect();
        watches.sort_by(|a, b| a.watch_id.cmp(&b.watch_id));
        watches
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum BashWatchInput {
    Register { contract: BashWatchContract },
    Cancel { watch_id: String },
    List,
}

pub struct BashWatchTool;

#[async_trait]
impl Tool for BashWatchTool {
    fn name(&self) -> &str {
        "bash_watch"
    }

    fn description(&self) -> String {
        "Register explicit bash wake contracts owned by the current work scope. Ordinary bash output and process exit remain passive unless a bash_watch contract exists. Watches record typed trigger intent, typed resume intent, creating conversation provenance, and route future wake delivery to the active continuation for the work scope.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": "Manage explicit bash watch contracts. Set op=register|cancel|list. Watches are owned by WorkScope, not by ordinary bash handle ownership.",
            "properties": {
                "op": { "type": "string", "enum": ["register", "cancel", "list"] },
                "watch_id": { "type": "string", "description": "Watch id for op=cancel." },
                "contract": {
                    "type": "object",
                    "description": "Typed wake contract for op=register.",
                    "properties": {
                        "trigger": {
                            "type": "object",
                            "description": "Why the watch should fire. This is explicit intent; bash handles alone do not imply a wake.",
                            "properties": {
                                "kind": { "type": "string", "enum": ["command_exited", "output_contains", "manual"] },
                                "handle": { "type": "string" },
                                "text": { "type": "string" }
                            },
                            "required": ["kind"]
                        },
                        "wake": {
                            "type": "object",
                            "properties": {
                                "kind": { "type": "string", "enum": ["resume_agent"] },
                                "instruction": { "type": "string", "description": "Instruction delivered to the active continuation when the watch fires." }
                            },
                            "required": ["kind", "instruction"]
                        }
                    },
                    "required": ["trigger", "wake"]
                }
            },
            "required": ["op"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed: BashWatchInput = match serde_json::from_value(input) {
            Ok(parsed) => parsed,
            Err(e) => return ToolOutput::error(format!("invalid bash_watch input: {e}")),
        };
        match parsed {
            BashWatchInput::Register { contract } => {
                let watch = ctx
                    .bash_watch_registry()
                    .insert(
                        ctx.work_scope.clone(),
                        ctx.conversation_id.clone(),
                        contract,
                    )
                    .await;
                ToolOutput::success(format!("registered bash watch {}", watch.watch_id))
                    .with_display(json!({ "watch": watch }))
            }
            BashWatchInput::Cancel { watch_id } => {
                match ctx.bash_watch_registry().remove(&watch_id).await {
                    Some(watch) if watch.work_scope == ctx.work_scope => {
                        ToolOutput::success(format!("cancelled bash watch {watch_id}"))
                    }
                    Some(watch) => {
                        ctx.bash_watch_registry()
                            .insert(
                                watch.work_scope,
                                watch.creating_conversation_id,
                                watch.contract,
                            )
                            .await;
                        ToolOutput::error("watch_not_found_in_current_work_scope")
                    }
                    None => ToolOutput::error("watch_not_found"),
                }
            }
            BashWatchInput::List => {
                let watches = ctx
                    .bash_watch_registry()
                    .list_for_scope(&ctx.work_scope)
                    .await;
                ToolOutput::success(format!("{} bash watch(es)", watches.len()))
                    .with_display(json!({ "work_scope": ctx.work_scope, "watches": watches }))
            }
        }
    }
}
