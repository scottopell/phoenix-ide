use serde::{Deserialize, Serialize};

pub const MAX_AUTHORITY_BYTES: usize = 16_384;

/// Complete local Coordinator standing instructions (REQ-AUTH-001/002).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CoordinatorAuthority(String);

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("Authority instructions exceed the maximum of 16,384 UTF-8 bytes")]
pub struct AuthorityTooLarge;

impl CoordinatorAuthority {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<String> for CoordinatorAuthority {
    type Error = AuthorityTooLarge;

    fn try_from(instructions: String) -> Result<Self, Self::Error> {
        if instructions.len() > MAX_AUTHORITY_BYTES {
            return Err(AuthorityTooLarge);
        }
        Ok(Self(instructions))
    }
}

impl From<CoordinatorAuthority> for String {
    fn from(authority: CoordinatorAuthority) -> Self {
        authority.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_authority_preserves_exact_text_and_empty_clear() {
        for text in ["", "  Preserve source.\n\nAllow unused cache cleanup. 🐦\n"] {
            let authority = CoordinatorAuthority::try_from(text.to_owned()).unwrap();
            assert_eq!(authority.as_str(), text);
            assert_eq!(authority.is_empty(), text.is_empty());
            assert_eq!(
                serde_json::from_str::<CoordinatorAuthority>(
                    &serde_json::to_string(&authority).unwrap()
                )
                .unwrap(),
                authority
            );
        }
    }

    #[test]
    fn coordinator_authority_bounds_utf8_bytes_in_constructor_and_serde() {
        let exact = "🐦".repeat(MAX_AUTHORITY_BYTES / 4);
        assert!(CoordinatorAuthority::try_from(exact.clone()).is_ok());
        let oversized = format!("{exact}a");
        assert!(CoordinatorAuthority::try_from(oversized.clone()).is_err());
        assert!(
            serde_json::from_value::<CoordinatorAuthority>(serde_json::json!(oversized)).is_err()
        );
        assert!(serde_json::from_value::<CoordinatorAuthority>(serde_json::json!(null)).is_err());
    }
}
