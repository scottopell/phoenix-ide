//! Instance-scoped activation shared by skill discovery and MCP configuration.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

/// An unavailable registry excludes all plugins. A loaded registry enables
/// unregistered packages and excludes exactly its disabled names.
#[derive(Clone, Debug, Default)]
pub struct PluginActivation {
    snapshot: Arc<RwLock<Option<Arc<HashSet<String>>>>>,
}

impl PluginActivation {
    /// Publish one complete, successfully read registry snapshot.
    ///
    /// # Panics
    /// Panics if the activation lock was poisoned by a prior panic.
    pub fn publish(&self, disabled: HashSet<String>) {
        *self
            .snapshot
            .write()
            .expect("plugin activation lock poisoned") = Some(Arc::new(disabled));
    }

    /// Capture a consistent activation decision for one discovery pass.
    ///
    /// # Panics
    /// Panics if the activation lock was poisoned by a prior panic.
    #[must_use]
    pub fn snapshot(&self) -> PluginActivationSnapshot {
        PluginActivationSnapshot(
            self.snapshot
                .read()
                .expect("plugin activation lock poisoned")
                .clone(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct PluginActivationSnapshot(Option<Arc<HashSet<String>>>);

impl PluginActivationSnapshot {
    #[must_use]
    pub fn is_enabled(&self, name: &str) -> bool {
        self.0
            .as_ref()
            .is_some_and(|disabled| !disabled.contains(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_instance_scoped_and_stable() {
        let first = PluginActivation::default();
        let second = PluginActivation::default();
        let reader = first.clone();
        assert!(!reader.snapshot().is_enabled("demo"));
        first.publish(HashSet::new());
        let captured = reader.snapshot();
        assert!(captured.is_enabled("demo"));
        assert!(!second.snapshot().is_enabled("demo"));
        first.publish(HashSet::from(["demo".into()]));
        assert!(!reader.snapshot().is_enabled("demo"));
        assert!(captured.is_enabled("demo"));
        first.publish(HashSet::new());
        assert!(reader.snapshot().is_enabled("demo"));
    }
}
