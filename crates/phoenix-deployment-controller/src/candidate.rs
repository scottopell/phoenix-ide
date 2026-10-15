#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FullCommitSha([u8; 20]);

impl FullCommitSha {
    /// # Errors
    /// Returns [`InvalidHexIdentity`] unless `value` is exactly 40 hexadecimal characters.
    pub fn parse(value: &str) -> Result<Self, InvalidHexIdentity> {
        Ok(Self(parse_hex::<20>(value)?))
    }

    #[must_use]
    pub const fn bytes(self) -> [u8; 20] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ArtifactDigest([u8; 32]);

impl ArtifactDigest {
    /// # Errors
    /// Returns [`InvalidHexIdentity`] unless `value` is exactly 64 hexadecimal characters.
    pub fn parse(value: &str) -> Result<Self, InvalidHexIdentity> {
        Ok(Self(parse_hex::<32>(value)?))
    }

    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RetainedCandidateIdentity {
    pub source_commit: FullCommitSha,
    pub artifact_digest: ArtifactDigest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidHexIdentity;

fn parse_hex<const N: usize>(value: &str) -> Result<[u8; N], InvalidHexIdentity> {
    if value.len() != N * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(InvalidHexIdentity);
    }

    let mut parsed = [0_u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(pair[0]).ok_or(InvalidHexIdentity)?;
        let low = hex_nibble(pair[1]).ok_or(InvalidHexIdentity)?;
        parsed[index] = (high << 4) | low;
    }
    Ok(parsed)
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}
