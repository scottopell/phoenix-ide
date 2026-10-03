use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SvgArtifactReference {
    pub artifact_id: String,
    pub conversation_id: String,
    pub title: String,
    pub description: String,
    pub width: f64,
    pub height: f64,
    pub validation: SvgValidationOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SvgValidationOutcome {
    AcceptedStaticSvg,
}
