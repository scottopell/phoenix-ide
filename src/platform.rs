//! Platform sandboxing capability detection.
//!
//! REQ-PROJ-013: Platform Capability Detection
//!
//! Probed once at server startup and threaded through `AppState` / `RuntimeManager`
//! so that mode-aware tool registries can adapt their tool sets.

/// Platform sandboxing capabilities detected at startup.
/// REQ-PROJ-013: Platform Capability Detection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlatformCapability {
    /// No kernel-level sandbox available — Explore mode uses restricted tool set
    None,
    /// Linux Landlock available (kernel >= 5.13, LSM enabled)
    Landlock,
    /// macOS sandbox-exec available
    MacOSSandbox,
}

impl PlatformCapability {
    /// Probe the current platform for sandboxing support.
    /// Called once at server startup.
    pub fn detect() -> Self {
        // On macOS, check for sandbox-exec
        if cfg!(target_os = "macos")
            && std::process::Command::new("sandbox-exec")
                .arg("-n")
                .arg("no-network") // test with a harmless profile
                .arg("true")
                .output()
                .is_ok()
        {
            return Self::MacOSSandbox;
        }

        // On Linux, check for Landlock
        if cfg!(target_os = "linux")
            && std::path::Path::new("/sys/kernel/security/landlock").exists()
        {
            return Self::Landlock;
        }

        Self::None
    }

    /// Whether a sandbox is available for read-only bash enforcement
    pub fn has_sandbox(self) -> bool {
        match self {
            Self::None => false,
            Self::Landlock | Self::MacOSSandbox => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_valid_variant() {
        let cap = PlatformCapability::detect();
        // Just verify it doesn't panic and returns a known variant
        let _ = cap.has_sandbox();
    }

    #[test]
    fn none_has_no_sandbox() {
        assert!(!PlatformCapability::None.has_sandbox());
    }

    #[test]
    fn landlock_has_sandbox() {
        assert!(PlatformCapability::Landlock.has_sandbox());
    }

    #[test]
    fn macos_sandbox_has_sandbox() {
        assert!(PlatformCapability::MacOSSandbox.has_sandbox());
    }
}
