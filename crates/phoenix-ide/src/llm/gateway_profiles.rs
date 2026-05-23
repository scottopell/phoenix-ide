//! Static capability profiles for LLM gateways that do not implement `/v1/models`.
//!
//! When [`super::discovery::discover_models`] returns an empty set for a
//! configured gateway, [`super::registry::ModelRegistry::new_with_discovery`]
//! consults this table to decide which hardcoded models to surface and which
//! `anthropic-beta` headers are safe to send. Gateways that do implement
//! `/v1/models` populate the registry from the discovery response and bypass
//! this table entirely.
//!
//! Adding a new gateway: append a `GatewayProfile` to [`PROFILES`] with a
//! host substring that uniquely identifies the gateway's URL and the
//! capability subset confirmed against that deployment.
//!
//! Adding a new model: if the model is confirmed to work on a profiled
//! gateway, add its id to that profile's `supported_model_ids`. Unknown
//! models default to hidden on profiled gateways — safer than the inverse,
//! which is what produced the prod bug this module exists to fix
//! (conversation `8f82c521`, 2026-04-24: exe.dev silently dropped requests
//! carrying the `context-1m-2025-08-07` beta header).

/// Capability profile for a known LLM gateway.
pub struct GatewayProfile {
    /// Case-insensitive substring matched against the configured gateway URL.
    /// The first matching profile in [`PROFILES`] wins, so substrings should be
    /// specific enough to avoid collisions (IPs and FQDNs are good choices).
    pub host_substring: &'static str,
    /// Hardcoded model IDs (from [`super::all_models`]) confirmed to work on
    /// this gateway. Models registered in [`super::all_models`] but absent
    /// here are filtered out of the model picker when this profile is active.
    pub supported_model_ids: &'static [&'static str],
    /// `anthropic-beta` header values this gateway is known to accept.
    /// Headers Phoenix would otherwise send are stripped from outbound
    /// Anthropic requests when this profile is active.
    pub supported_beta_headers: &'static [&'static str],
}

/// Known gateway profiles. First host-substring match wins.
const PROFILES: &[GatewayProfile] = &[
    // exe.dev built-in gateway. Does not implement `/anthropic/v1/models`
    // (see `super::registry::new_with_discovery`). Does not accept the
    // `context-1m-2025-08-07` beta header — requests carrying it return a
    // 200 with an empty SSE stream rather than a 4xx, producing a silent
    // retry loop. See task 24695.
    GatewayProfile {
        host_substring: "169.254.169.254",
        supported_model_ids: &[
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-opus-4-5",
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.3-codex",
            "mock",
        ],
        supported_beta_headers: &["advanced-tool-use-2025-11-20"],
    },
];

/// Look up the profile matching the given gateway URL, if any.
///
/// Matching is a case-insensitive substring check against the full URL. The
/// host substrings in [`PROFILES`] are chosen to be unambiguous (IPs, FQDNs)
/// so substring matching is sufficient without a URL parser dependency.
pub fn match_profile(gateway_url: &str) -> Option<&'static GatewayProfile> {
    let url_lower = gateway_url.to_lowercase();
    PROFILES
        .iter()
        .find(|p| url_lower.contains(&p.host_substring.to_lowercase()))
}

impl GatewayProfile {
    /// True if `model_id` is permitted by this profile.
    pub fn allows_model(&self, model_id: &str) -> bool {
        self.supported_model_ids.contains(&model_id)
    }

    /// True if `beta_header` is permitted by this profile. Used to filter
    /// outbound `anthropic-beta` header values at request-build time.
    pub fn allows_beta_header(&self, beta_header: &str) -> bool {
        self.supported_beta_headers.contains(&beta_header)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_exe_dev_by_ip_substring() {
        let profile = match_profile("http://169.254.169.254/gateway/llm")
            .expect("exe.dev profile should match its canonical URL");
        assert_eq!(profile.host_substring, "169.254.169.254");
    }

    #[test]
    fn matches_exe_dev_with_trailing_slash() {
        assert!(match_profile("http://169.254.169.254/gateway/llm/").is_some());
    }

    #[test]
    fn match_is_case_insensitive() {
        assert!(match_profile("HTTP://169.254.169.254/Gateway/LLM").is_some());
    }

    #[test]
    fn unknown_gateway_returns_none() {
        assert!(match_profile("https://example.com/v1").is_none());
        assert!(match_profile("https://api.anthropic.com").is_none());
    }

    #[test]
    fn exe_dev_excludes_1m_variants() {
        let profile = match_profile("http://169.254.169.254/gateway/llm").unwrap();
        assert!(!profile.allows_model("claude-opus-4-7-1m"));
        assert!(!profile.allows_model("claude-opus-4-6-1m"));
        assert!(!profile.allows_model("claude-sonnet-4-6-1m"));
    }

    #[test]
    fn exe_dev_allows_base_variants() {
        let profile = match_profile("http://169.254.169.254/gateway/llm").unwrap();
        assert!(profile.allows_model("claude-opus-4-7"));
        assert!(profile.allows_model("claude-sonnet-4-6"));
        assert!(profile.allows_model("gpt-5.5"));
    }

    #[test]
    fn exe_dev_blocks_context_1m_beta_header() {
        let profile = match_profile("http://169.254.169.254/gateway/llm").unwrap();
        assert!(!profile.allows_beta_header("context-1m-2025-08-07"));
        assert!(profile.allows_beta_header("advanced-tool-use-2025-11-20"));
    }
}
