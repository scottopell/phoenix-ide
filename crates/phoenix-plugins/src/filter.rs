//! Process-wide enabled/disabled plugin filter (REQ-AP-008).
//!
//! The disabled-plugin set is user-level harness state shared by every
//! consumer of plugin contributions: skill discovery (sync, per-turn) and
//! the MCP config merge (async, per-reload). Both must observe the same
//! set or a disabled plugin would vanish from one surface and persist on
//! the other. The runtime seeds this filter from the persisted plugins
//! registry at startup and after every lifecycle change; absent names are
//! enabled (the default).

use std::collections::HashSet;
use std::sync::RwLock;

static DISABLED: RwLock<Option<HashSet<String>>> = RwLock::new(None);

/// Replace the disabled set (seeded from persisted registry state).
///
/// # Panics
/// Panics when the internal lock is poisoned (a prior holder panicked).
pub fn set_disabled(disabled: impl IntoIterator<Item = String>) {
    let mut guard = DISABLED.write().expect("plugin filter lock poisoned");
    *guard = Some(disabled.into_iter().collect());
}

/// Whether a plugin name is disabled. Absent filter state means every
/// plugin is enabled.
///
/// # Panics
/// Panics when the internal lock is poisoned (a prior holder panicked).
#[must_use]
pub fn is_disabled(name: &str) -> bool {
    let guard = DISABLED.read().expect("plugin filter lock poisoned");
    guard.as_ref().is_some_and(|set| set.contains(name))
}

/// The current disabled set (empty when never seeded).
///
/// # Panics
/// Panics when the internal lock is poisoned (a prior holder panicked).
#[must_use]
pub fn disabled_set() -> std::collections::HashSet<String> {
    let guard = DISABLED.read().expect("plugin filter lock poisoned");
    guard.clone().unwrap_or_default()
}

/// Test helper: clear the filter back to never-seeded state. Visible to
/// dependent crates' test builds (skill and MCP integration tests reset
/// the process-global filter between cases); never called by production
/// code.
#[doc(hidden)]
#[allow(clippy::missing_panics_doc)]
pub fn reset_for_tests() {
    let mut guard = DISABLED.write().expect("plugin filter lock poisoned");
    *guard = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_filter_means_everything_enabled() {
        reset_for_tests();
        assert!(!is_disabled("anything"));
        assert!(disabled_set().is_empty());
    }

    #[test]
    fn seeded_filter_disables_named_plugins_only() {
        reset_for_tests();
        set_disabled(["devtools".to_string()]);
        assert!(is_disabled("devtools"));
        assert!(!is_disabled("other"));
        assert_eq!(disabled_set().len(), 1);
        reset_for_tests();
    }
}
