use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivePrIdentity {
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivePrSelectionProvenance {
    Inferred,
    Pinned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivePrSelection {
    pub pr: ActivePrIdentity,
    pub provenance: ActivePrSelectionProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivePrBranchContext {
    pub repository_identity: String,
    pub branch_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivePrInferenceInput {
    pub latest_observed_branch: Option<ActivePrBranchContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivePrSelectionState {
    pub selection: Option<ActivePrSelection>,
    pub inference_generation: u64,
}
