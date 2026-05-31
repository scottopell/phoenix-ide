//! Kill signal the agent may request when terminating a bash handle.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Kill signal the agent may request. Sent EXACTLY ONCE per kill call —
/// no auto-escalation (REQ-BASH-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "UPPERCASE")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum KillSignal {
    Term,
    Kill,
}

impl KillSignal {
    /// Stable string identifier used in tool responses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            KillSignal::Term => "TERM",
            KillSignal::Kill => "KILL",
        }
    }

    #[cfg(unix)]
    #[must_use]
    pub fn as_libc(self) -> i32 {
        match self {
            KillSignal::Term => libc::SIGTERM,
            KillSignal::Kill => libc::SIGKILL,
        }
    }
}
