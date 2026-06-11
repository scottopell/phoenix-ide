//! Platform sandboxing capability detection.
//!
//! REQ-PROJ-013: Platform Capability Detection
//!
//! Probed once at server startup and threaded through `AppState` / `RuntimeManager`
//! so that mode-aware tool registries can adapt their tool sets.

/// Platform sandboxing capabilities detected at startup.
/// REQ-PROJ-013: Platform Capability Detection
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformCapability {
    /// No kernel-level sandbox available — Explore mode uses restricted tool set
    None { details: String },
    /// `nono` reports an enforceable backend for this host.
    Nono { platform: String, details: String },
}

impl PlatformCapability {
    /// Probe the current platform for sandboxing support.
    /// Called once at server startup.
    #[must_use]
    pub fn detect() -> Self {
        let support = nono::Sandbox::support_info();
        if support.is_supported {
            Self::Nono {
                platform: support.platform.to_string(),
                details: support.details,
            }
        } else {
            Self::None {
                details: support.details,
            }
        }
    }

    /// Whether a sandbox is available for read-only bash enforcement.
    #[must_use]
    pub fn has_sandbox(&self) -> bool {
        matches!(self, Self::Nono { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_valid_variant() {
        let cap = PlatformCapability::detect();
        let _ = cap.has_sandbox();
    }

    #[test]
    fn none_has_no_sandbox() {
        assert!(!PlatformCapability::None {
            details: "unsupported".to_string()
        }
        .has_sandbox());
    }

    #[test]
    fn nono_has_sandbox() {
        assert!(PlatformCapability::Nono {
            platform: "test".to_string(),
            details: "supported".to_string()
        }
        .has_sandbox());
    }
}
