use rustls_pki_types::pem::PemObject as _;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
pub enum InstanceIdParseError {
    #[error("invalid UUID: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("instance identity must be UUIDv4")]
    NotVersionFour,
    #[error("instance identity must use the RFC 4122 variant")]
    NotRfc4122,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(uuid::Uuid);

impl InstanceId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for InstanceId {
    type Err = InstanceIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let uuid = uuid::Uuid::parse_str(value)?;
        if uuid.get_variant() != uuid::Variant::RFC4122 {
            return Err(InstanceIdParseError::NotRfc4122);
        }
        if uuid.get_version_num() != 4 {
            return Err(InstanceIdParseError::NotVersionFour);
        }
        Ok(Self(uuid))
    }
}

impl serde::Serialize for InstanceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for InstanceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerBaseUrl(url::Url);

#[derive(Debug, thiserror::Error)]
pub enum PeerBaseUrlError {
    #[error("invalid peer URL: {0}")]
    Invalid(#[from] url::ParseError),
    #[error("peer URL must use HTTPS")]
    NotHttps,
    #[error("peer URL must not contain credentials, query, or fragment")]
    ContainsAmbientData,
    #[error("peer URL must be an origin without a path")]
    ContainsPath,
}

impl FromStr for PeerBaseUrl {
    type Err = PeerBaseUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let url = url::Url::parse(value)?;
        if url.scheme() != "https" {
            return Err(PeerBaseUrlError::NotHttps);
        }
        let Some(host) = url.host() else {
            return Err(PeerBaseUrlError::ContainsAmbientData);
        };
        if matches!(host, url::Host::Domain(domain) if !domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
        {
            return Err(PeerBaseUrlError::ContainsAmbientData);
        }
        if url.port() == Some(0) {
            return Err(PeerBaseUrlError::ContainsAmbientData);
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(PeerBaseUrlError::ContainsAmbientData);
        }
        if url.path() != "/" {
            return Err(PeerBaseUrlError::ContainsPath);
        }
        Ok(Self(url))
    }
}

impl fmt::Display for PeerBaseUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerHost<'a> {
    DomainOrIpv4(&'a str),
    Ipv6(std::net::Ipv6Addr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationQueryDatabaseEndpoint(url::Url);

impl FederationQueryDatabaseEndpoint {
    #[must_use]
    pub fn as_url(&self) -> &url::Url {
        &self.0
    }
}

impl PeerBaseUrl {
    /// Return the typed normalized host.
    ///
    /// # Panics
    /// Panics only if this value bypassed `PeerBaseUrl` construction.
    #[must_use]
    pub fn host(&self) -> PeerHost<'_> {
        match self
            .0
            .host()
            .expect("validated HTTPS origin always has a host")
        {
            url::Host::Domain(domain) => PeerHost::DomainOrIpv4(domain),
            url::Host::Ipv4(_address) => PeerHost::DomainOrIpv4(
                self.0
                    .host_str()
                    .expect("validated IPv4 origin always has a host"),
            ),
            url::Host::Ipv6(address) => PeerHost::Ipv6(address),
        }
    }

    /// Return the explicit or HTTPS-default port.
    ///
    /// # Panics
    /// Panics only if this value bypassed `PeerBaseUrl` construction.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.0
            .port_or_known_default()
            .expect("HTTPS always has a known default port")
    }

    /// Return the closed endpoint for the bounded remote database query operation.
    ///
    /// # Panics
    /// Panics only if the fixed compile-time route is not a valid URL path.
    #[must_use]
    pub fn query_database_endpoint(&self) -> FederationQueryDatabaseEndpoint {
        FederationQueryDatabaseEndpoint(
            self.0
                .join("/api/federation/peer/query-database")
                .expect("fixed federation route is a valid URL path"),
        )
    }

    /// Reconstruct a peer origin from normalized relational columns.
    ///
    /// # Errors
    /// Returns an error if the persisted host and port do not form a bare HTTPS origin.
    pub fn from_host_port(host: PeerHost<'_>, port: u16) -> Result<Self, PeerBaseUrlError> {
        match host {
            PeerHost::DomainOrIpv4(host) => Self::from_str(&format!("https://{host}:{port}")),
            PeerHost::Ipv6(address) => Self::from_str(&format!("https://[{address}]:{port}")),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PeerBearerCredential(String);

#[derive(Debug, thiserror::Error)]
#[error("invalid peer bearer credential")]
pub struct PeerBearerCredentialError;

impl PeerBearerCredential {
    /// Parse a receiver-issued federation bearer credential.
    ///
    /// # Errors
    /// Returns an error when the credential is outside the reserved token namespace or shape.
    pub fn parse(value: String) -> Result<Self, PeerBearerCredentialError> {
        let encoded = value
            .strip_prefix("phx_peer_")
            .ok_or(PeerBearerCredentialError)?;
        if encoded.len() != 43
            || !encoded
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(PeerBearerCredentialError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

const MAX_PEER_CA_CERTIFICATE_PEM_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeerTlsTrust {
    PlatformRoots,
    PrivateCa {
        certificate_pem: PeerCaCertificatePem,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerCaCertificatePem(String);

impl PeerCaCertificatePem {
    /// Parse one bounded public CA certificate in PEM form.
    ///
    /// # Errors
    /// Returns an error when the value is empty, oversized, malformed, or contains anything other
    /// than one certificate.
    pub fn parse(value: impl Into<String>) -> Result<Self, PeerCaCertificatePemError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PeerCaCertificatePemError::Empty);
        }
        if value.len() > MAX_PEER_CA_CERTIFICATE_PEM_BYTES {
            return Err(PeerCaCertificatePemError::TooLarge);
        }
        let trimmed = value.trim();
        let Some(after_begin) = trimmed.strip_prefix("-----BEGIN CERTIFICATE-----") else {
            return Err(PeerCaCertificatePemError::CertificateCount);
        };
        if !trimmed.ends_with("-----END CERTIFICATE-----") || after_begin.contains("-----BEGIN ") {
            return Err(PeerCaCertificatePemError::CertificateCount);
        }
        let certificates = rustls_pki_types::CertificateDer::pem_slice_iter(trimmed.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PeerCaCertificatePemError::Invalid)?;
        if certificates.len() != 1 {
            return Err(PeerCaCertificatePemError::CertificateCount);
        }
        let [certificate]: [_; 1] = certificates
            .try_into()
            .map_err(|_| PeerCaCertificatePemError::CertificateCount)?;
        validate_peer_ca_certificate(certificate.as_ref())?;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(certificate)
            .map_err(|_| PeerCaCertificatePemError::Invalid)?;
        Ok(Self(format!("{trimmed}\n")))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

fn validate_peer_ca_certificate(der: &[u8]) -> Result<(), PeerCaCertificatePemError> {
    use x509_parser::prelude::FromDer as _;

    let (remainder, certificate) = x509_parser::certificate::X509Certificate::from_der(der)
        .map_err(|_| PeerCaCertificatePemError::Invalid)?;
    let basic_constraints = certificate
        .get_extension_unique(&x509_parser::oid_registry::OID_X509_EXT_BASIC_CONSTRAINTS)
        .map_err(|_| PeerCaCertificatePemError::Invalid)?
        .ok_or(PeerCaCertificatePemError::NotCertificateAuthority)?;
    if !remainder.is_empty() {
        return Err(PeerCaCertificatePemError::Invalid);
    }
    validate_basic_constraints_der(basic_constraints.value)?;
    if let Some(key_usage) = certificate
        .get_extension_unique(&x509_parser::oid_registry::OID_X509_EXT_KEY_USAGE)
        .map_err(|_| PeerCaCertificatePemError::Invalid)?
    {
        validate_key_usage_der(key_usage.value)?;
        let (key_usage_remainder, key_usage) =
            x509_parser::extensions::KeyUsage::from_der(key_usage.value)
                .map_err(|_| PeerCaCertificatePemError::Invalid)?;
        if !key_usage_remainder.is_empty() {
            return Err(PeerCaCertificatePemError::Invalid);
        }
        if !key_usage.key_cert_sign() {
            return Err(PeerCaCertificatePemError::NotCertificateAuthority);
        }
    }
    Ok(())
}

fn validate_basic_constraints_der(value: &[u8]) -> Result<(), PeerCaCertificatePemError> {
    let (sequence, remainder) = strict_der_value(value, 0x30)?;
    if !remainder.is_empty() {
        return Err(PeerCaCertificatePemError::Invalid);
    }
    if sequence.is_empty() {
        return Err(PeerCaCertificatePemError::NotCertificateAuthority);
    }
    if !sequence.starts_with(&[0x01, 0x01, 0xff]) {
        return Err(PeerCaCertificatePemError::Invalid);
    }
    validate_optional_path_length(&sequence[3..])
}

fn validate_optional_path_length(remainder: &[u8]) -> Result<(), PeerCaCertificatePemError> {
    if remainder.is_empty() {
        return Ok(());
    }
    let (path_length, remainder) = strict_der_value(remainder, 0x02)?;
    if !remainder.is_empty()
        || path_length.is_empty()
        || path_length[0] & 0x80 != 0
        || (path_length.len() > 1 && path_length[0] == 0 && path_length[1] & 0x80 == 0)
    {
        return Err(PeerCaCertificatePemError::Invalid);
    }
    Ok(())
}

fn validate_key_usage_der(value: &[u8]) -> Result<(), PeerCaCertificatePemError> {
    let (bit_string, remainder) = strict_der_value(value, 0x03)?;
    if !remainder.is_empty() || !(2..=3).contains(&bit_string.len()) {
        return Err(PeerCaCertificatePemError::Invalid);
    }
    let unused_bits = bit_string[0];
    let last = *bit_string
        .last()
        .ok_or(PeerCaCertificatePemError::Invalid)?;
    if unused_bits > 7
        || last == 0
        || unused_bits != u8::try_from(last.trailing_zeros()).unwrap_or(u8::MAX)
    {
        return Err(PeerCaCertificatePemError::Invalid);
    }
    Ok(())
}

fn strict_der_value(
    input: &[u8],
    expected_tag: u8,
) -> Result<(&[u8], &[u8]), PeerCaCertificatePemError> {
    let (&tag, after_tag) = input
        .split_first()
        .ok_or(PeerCaCertificatePemError::Invalid)?;
    if tag != expected_tag {
        return Err(PeerCaCertificatePemError::Invalid);
    }
    let (&first_length, after_first_length) = after_tag
        .split_first()
        .ok_or(PeerCaCertificatePemError::Invalid)?;
    let (length, content) = if first_length & 0x80 == 0 {
        (usize::from(first_length), after_first_length)
    } else {
        let length_octets = usize::from(first_length & 0x7f);
        if length_octets == 0 || length_octets > std::mem::size_of::<usize>() {
            return Err(PeerCaCertificatePemError::Invalid);
        }
        if after_first_length.len() < length_octets {
            return Err(PeerCaCertificatePemError::Invalid);
        }
        let (encoded_length, content) = after_first_length.split_at(length_octets);
        if encoded_length[0] == 0 {
            return Err(PeerCaCertificatePemError::Invalid);
        }
        let length = encoded_length.iter().try_fold(0usize, |length, octet| {
            length
                .checked_mul(256)
                .and_then(|length| length.checked_add(usize::from(*octet)))
        });
        let length = length.ok_or(PeerCaCertificatePemError::Invalid)?;
        if length < 128 {
            return Err(PeerCaCertificatePemError::Invalid);
        }
        (length, content)
    };
    if content.len() < length {
        return Err(PeerCaCertificatePemError::Invalid);
    }
    Ok(content.split_at(length))
}

impl serde::Serialize for PeerCaCertificatePem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> serde::Deserialize<'de> for PeerCaCertificatePem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PeerCaCertificatePemError {
    #[error("peer CA certificate PEM must not be empty")]
    Empty,
    #[error("peer CA certificate PEM exceeds 32 KiB")]
    TooLarge,
    #[error("peer CA certificate PEM is not a certificate authority")]
    NotCertificateAuthority,
    #[error("peer CA certificate PEM is invalid")]
    Invalid,
    #[error("peer CA certificate PEM must contain exactly one certificate")]
    CertificateCount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationCredentialVerifier([u8; 32]);

impl FederationCredentialVerifier {
    #[must_use]
    pub fn from_bearer(bearer: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        Self(Sha256::digest(bearer).into())
    }
}

impl fmt::Display for FederationCredentialVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_base_url_requires_a_bare_https_origin() {
        assert!(PeerBaseUrl::from_str("https://peer.example").is_ok());
        let endpoint = PeerBaseUrl::from_str("https://peer.example:8443")
            .unwrap()
            .query_database_endpoint();
        assert_eq!(
            endpoint.as_url().as_str(),
            "https://peer.example:8443/api/federation/peer/query-database"
        );
        let ipv6 = PeerBaseUrl::from_str("https://[2001:db8::1]:8031").unwrap();
        assert_eq!(ipv6.host(), PeerHost::Ipv6("2001:db8::1".parse().unwrap()));
        assert_eq!(
            PeerBaseUrl::from_str("https://127.0.0.1").unwrap().host(),
            PeerHost::DomainOrIpv4("127.0.0.1")
        );
        assert_eq!(
            PeerBaseUrl::from_host_port(PeerHost::Ipv6("2001:db8::1".parse().unwrap()), 8031)
                .unwrap(),
            ipv6
        );
        assert_eq!(
            ipv6.query_database_endpoint().as_url().as_str(),
            "https://[2001:db8::1]:8031/api/federation/peer/query-database"
        );
        for value in [
            "http://peer.example",
            "https://user@peer.example",
            "https://peer.example/path",
            "https://peer.example/?query=yes",
            "https://peer.example/#fragment",
            "https://peer.example:0",
        ] {
            assert!(PeerBaseUrl::from_str(value).is_err(), "{value}");
        }
    }

    #[test]
    fn peer_bearer_credential_requires_issued_shape() {
        assert!(PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).is_ok());
        for value in [
            "owner-password".to_string(),
            "phx_peer_short".to_string(),
            format!("phx_peer_{}=", "a".repeat(42)),
        ] {
            assert!(PeerBearerCredential::parse(value).is_err());
        }
    }

    #[test]
    fn peer_private_ca_requires_one_bounded_certificate() {
        let temp = tempfile::tempdir().unwrap();
        let paths = phoenix_tls::ensure_ca(temp.path()).unwrap();
        let certificate = std::fs::read_to_string(paths.cert_path).unwrap();
        assert_eq!(
            PeerCaCertificatePem::parse(&certificate).unwrap().expose(),
            certificate
        );
        assert_eq!(
            PeerCaCertificatePem::parse(" ").unwrap_err(),
            PeerCaCertificatePemError::Empty
        );
        assert_eq!(
            PeerCaCertificatePem::parse(format!("{certificate}{certificate}")).unwrap_err(),
            PeerCaCertificatePemError::CertificateCount
        );
        let private_key = std::fs::read_to_string(paths.key_path).unwrap();
        assert_eq!(
            PeerCaCertificatePem::parse(format!("{certificate}{private_key}")).unwrap_err(),
            PeerCaCertificatePemError::CertificateCount
        );
        assert_eq!(
            PeerCaCertificatePem::parse(format!("{certificate}not PEM")).unwrap_err(),
            PeerCaCertificatePemError::CertificateCount
        );
        assert_eq!(
            PeerCaCertificatePem::parse(
                "-----BEGIN CERTIFICATE-----\nnot-base64\n-----END CERTIFICATE-----"
            )
            .unwrap_err(),
            PeerCaCertificatePemError::Invalid
        );
        let leaf_cert_path = temp.path().join("leaf.pem");
        let leaf_key_path = temp.path().join("leaf-key.pem");
        phoenix_tls::issue_leaf(
            temp.path(),
            &leaf_cert_path,
            &leaf_key_path,
            &["localhost".to_string()],
        )
        .unwrap();
        assert_eq!(
            PeerCaCertificatePem::parse(std::fs::read_to_string(leaf_cert_path).unwrap())
                .unwrap_err(),
            PeerCaCertificatePemError::NotCertificateAuthority
        );
        let mut ca_without_key_cert_sign = rcgen::CertificateParams::new(Vec::<String>::new())
            .expect("empty SAN list is valid for CA certificates");
        ca_without_key_cert_sign.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_without_key_cert_sign
            .key_usages
            .push(rcgen::KeyUsagePurpose::DigitalSignature);
        let key_pair = rcgen::KeyPair::generate().unwrap();
        assert_eq!(
            PeerCaCertificatePem::parse(
                ca_without_key_cert_sign
                    .self_signed(&key_pair)
                    .unwrap()
                    .pem(),
            )
            .unwrap_err(),
            PeerCaCertificatePemError::NotCertificateAuthority
        );
    }

    #[test]
    fn peer_private_ca_rejects_trailing_der_in_relevant_extensions() {
        let key_pair = rcgen::KeyPair::generate().unwrap();
        for (oid, content) in [
            (
                &[2, 5, 29, 15][..],
                &[0x03, 0x02, 0x02, 0x04, 0x05, 0x00][..],
            ),
            (
                &[2, 5, 29, 19][..],
                &[0x30, 0x03, 0x01, 0x01, 0xff, 0x05, 0x00][..],
            ),
        ] {
            let mut params = rcgen::CertificateParams::new(Vec::<String>::new())
                .expect("empty SAN list is valid for CA certificates");
            params.is_ca = if oid == [2, 5, 29, 19] {
                rcgen::IsCa::NoCa
            } else {
                rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained)
            };
            params
                .custom_extensions
                .push(rcgen::CustomExtension::from_oid_content(
                    oid,
                    content.to_vec(),
                ));
            assert_eq!(
                PeerCaCertificatePem::parse(params.self_signed(&key_pair).unwrap().pem())
                    .unwrap_err(),
                PeerCaCertificatePemError::Invalid
            );
        }
    }

    #[test]
    fn peer_private_ca_rejects_non_der_basic_constraints() {
        let key_pair = rcgen::KeyPair::generate().unwrap();
        for content in [
            vec![0x30, 0x03, 0x21, 0x01, 0xff],
            vec![0x30, 0x03, 0x01, 0x01, 0x01],
        ] {
            let mut params = rcgen::CertificateParams::new(Vec::<String>::new())
                .expect("empty SAN list is valid for CA certificates");
            params.is_ca = rcgen::IsCa::NoCa;
            params
                .custom_extensions
                .push(rcgen::CustomExtension::from_oid_content(
                    &[2, 5, 29, 19],
                    content,
                ));
            assert_eq!(
                PeerCaCertificatePem::parse(params.self_signed(&key_pair).unwrap().pem())
                    .unwrap_err(),
                PeerCaCertificatePemError::Invalid
            );
        }
    }

    #[test]
    fn instance_identity_rejects_non_v4_uuids() {
        for value in [
            "d9428888-122b-11e1-b85c-61cd3cbb3210",
            "21f7f8de-8051-5b89-8680-0195ef798b6a",
        ] {
            assert!(matches!(
                InstanceId::from_str(value),
                Err(InstanceIdParseError::NotVersionFour)
            ));
        }
        for value in [
            "00000000-0000-0000-0000-000000000000",
            "00000000-0000-4000-0000-000000000000",
        ] {
            assert!(matches!(
                InstanceId::from_str(value),
                Err(InstanceIdParseError::NotRfc4122)
            ));
        }
        assert!(InstanceId::from_str(&InstanceId::new().to_string()).is_ok());
    }
}
