use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};
use time::OffsetDateTime;

const DAY_SECONDS: i64 = 86_400;
const LEAF_VALID_DAYS: i64 = 397;
const CA_VALID_DAYS: i64 = 3_650;

#[derive(Debug, Clone)]
pub struct CertKeyPaths {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CaPaths {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

/// Compute the canonical paths for the local CA's cert and private key inside
/// `dir`. Returns the paths regardless of whether the files exist.
#[must_use]
pub fn ca_paths(dir: &Path) -> CaPaths {
    CaPaths {
        cert_path: dir.join("phoenix-local-ca.pem"),
        key_path: dir.join("phoenix-local-ca-key.pem"),
    }
}

/// Ensure the local CA cert + key exist in `dir`, generating a fresh CA if
/// neither file is present and returning the canonical paths in either case.
///
/// # Errors
///
/// - The directory cannot be created or read.
/// - Exactly one of the cert/key files exists (refuses to silently overwrite
///   a half-deleted CA).
/// - Key generation, serialisation, or atomic write fails.
pub fn ensure_ca(dir: &Path) -> Result<CaPaths, Box<dyn Error>> {
    fs::create_dir_all(dir)?;
    let paths = ca_paths(dir);
    match (paths.cert_path.exists(), paths.key_path.exists()) {
        (true, true) => Ok(paths),
        (false, false) => {
            write_ca(&paths.cert_path, &paths.key_path)?;
            Ok(paths)
        }
        _ => Err(format!(
            "managed TLS CA is incomplete in {}; remove both CA files or restore the missing one",
            dir.display()
        )
        .into()),
    }
}

/// Issue a leaf TLS certificate signed by the CA in `ca_dir`, writing the
/// cert and key PEM files to the provided paths. The CA is created on demand
/// if missing.
///
/// # Errors
///
/// - `hosts` is empty.
/// - The CA cannot be ensured (see [`ensure_ca`]).
/// - The cert/key parent directories cannot be created.
/// - Cert generation, signing, or atomic write fails.
pub fn issue_leaf(
    ca_dir: &Path,
    cert_path: &Path,
    key_path: &Path,
    hosts: &[String],
) -> Result<CertKeyPaths, Box<dyn Error>> {
    if hosts.is_empty() {
        return Err("at least one TLS host is required".into());
    }

    let ca = ensure_ca(ca_dir)?;
    if let Some(parent) = cert_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)?;
    }

    write_leaf(&ca.cert_path, &ca.key_path, cert_path, key_path, hosts)?;
    Ok(CertKeyPaths {
        cert_path: cert_path.to_path_buf(),
        key_path: key_path.to_path_buf(),
    })
}

fn write_ca(cert_path: &Path, key_path: &Path) -> Result<(), Box<dyn Error>> {
    let mut params =
        CertificateParams::new(Vec::<String>::new()).expect("empty SAN list is valid for CA certs");
    let (not_before, not_after) = validity_window(CA_VALID_DAYS)?;
    params.not_before = not_before;
    params.not_after = not_after;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, "Phoenix IDE Local CA");
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params.key_usages.push(KeyUsagePurpose::KeyCertSign);
    params.key_usages.push(KeyUsagePurpose::CrlSign);

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;
    write_pem(cert_path, &cert.pem(), PemKind::Public)?;
    write_pem(key_path, &key_pair.serialize_pem(), PemKind::Private)?;
    Ok(())
}

fn write_leaf(
    ca_cert_path: &Path,
    ca_key_path: &Path,
    cert_path: &Path,
    key_path: &Path,
    hosts: &[String],
) -> Result<(), Box<dyn Error>> {
    let ca_cert_pem = fs::read_to_string(ca_cert_path)?;
    let ca_key_pem = fs::read_to_string(ca_key_path)?;
    let ca_key = KeyPair::from_pem(&ca_key_pem)?;
    let issuer = Issuer::from_ca_cert_pem(&ca_cert_pem, ca_key)?;

    let mut params = CertificateParams::new(hosts.to_vec())?;
    let (not_before, not_after) = validity_window(LEAF_VALID_DAYS)?;
    params.not_before = not_before;
    params.not_after = not_after;
    params
        .distinguished_name
        .push(DnType::CommonName, "Phoenix IDE local HTTPS");
    params.use_authority_key_identifier_extension = true;
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ServerAuth);

    let key_pair = KeyPair::generate()?;
    let cert = params.signed_by(&key_pair, &issuer)?;
    write_pem(cert_path, &cert.pem(), PemKind::Public)?;
    write_pem(key_path, &key_pair.serialize_pem(), PemKind::Private)?;
    Ok(())
}

fn validity_window(valid_days: i64) -> Result<(OffsetDateTime, OffsetDateTime), Box<dyn Error>> {
    let now = OffsetDateTime::now_utc();
    let not_before = now
        .checked_sub(time::Duration::seconds(DAY_SECONDS))
        .ok_or("certificate not_before underflow")?;
    let not_after = now
        .checked_add(time::Duration::seconds(DAY_SECONDS * valid_days))
        .ok_or("certificate not_after overflow")?;
    Ok((not_before, not_after))
}

#[derive(Copy, Clone)]
enum PemKind {
    Public,
    Private,
}

fn write_pem(path: &Path, contents: &str, kind: PemKind) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mode = match kind {
            PemKind::Public => 0o644,
            PemKind::Private => 0o600,
        };
        // Create with the target mode up front so a private key is never briefly
        // world-readable in the window between creation under the process umask
        // and a follow-up chmod. `mode` only applies when O_CREAT makes a new
        // file, so re-assert it afterwards to also tighten a pre-existing file.
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(path)?;
        f.write_all(contents.as_bytes())?;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }

    #[cfg(not(unix))]
    {
        let _ = kind;
        fs::write(path, contents)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn ca_paths_uses_canonical_filenames_under_dir() {
        let dir = Path::new("/some/base/dir");
        let paths = ca_paths(dir);

        assert_eq!(paths.cert_path, dir.join("phoenix-local-ca.pem"));
        assert_eq!(paths.key_path, dir.join("phoenix-local-ca-key.pem"));
        assert!(paths
            .cert_path
            .to_string_lossy()
            .ends_with("phoenix-local-ca.pem"));
        assert!(paths
            .key_path
            .to_string_lossy()
            .ends_with("phoenix-local-ca-key.pem"));
    }

    #[test]
    fn ensure_ca_creates_both_files_on_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let paths = ensure_ca(tmp.path()).unwrap();

        assert!(paths.cert_path.exists(), "CA cert should exist");
        assert!(paths.key_path.exists(), "CA key should exist");
    }

    #[test]
    fn ensure_ca_is_idempotent_and_does_not_regenerate() {
        let tmp = TempDir::new().unwrap();
        let first = ensure_ca(tmp.path()).unwrap();
        let cert_bytes = fs::read(&first.cert_path).unwrap();
        let key_bytes = fs::read(&first.key_path).unwrap();

        let second = ensure_ca(tmp.path()).unwrap();

        assert_eq!(first.cert_path, second.cert_path);
        assert_eq!(first.key_path, second.key_path);
        assert_eq!(
            cert_bytes,
            fs::read(&second.cert_path).unwrap(),
            "CA cert must not be regenerated"
        );
        assert_eq!(
            key_bytes,
            fs::read(&second.key_path).unwrap(),
            "CA key must not be regenerated"
        );
    }

    #[test]
    fn ensure_ca_errors_when_only_cert_exists() {
        let tmp = TempDir::new().unwrap();
        let paths = ca_paths(tmp.path());
        fs::write(&paths.cert_path, "not a real cert").unwrap();

        assert!(ensure_ca(tmp.path()).is_err());
    }

    #[test]
    fn ensure_ca_errors_when_only_key_exists() {
        let tmp = TempDir::new().unwrap();
        let paths = ca_paths(tmp.path());
        fs::write(&paths.key_path, "not a real key").unwrap();

        assert!(ensure_ca(tmp.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn ensure_ca_sets_unix_permissions() {
        let tmp = TempDir::new().unwrap();
        let paths = ensure_ca(tmp.path()).unwrap();

        assert_eq!(mode_of(&paths.key_path), 0o600, "CA key should be 0o600");
        assert_eq!(mode_of(&paths.cert_path), 0o644, "CA cert should be 0o644");
    }

    #[test]
    fn issue_leaf_errors_on_empty_hosts() {
        let tmp = TempDir::new().unwrap();
        let cert_path = tmp.path().join("leaf.pem");
        let key_path = tmp.path().join("leaf-key.pem");

        let result = issue_leaf(tmp.path(), &cert_path, &key_path, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn issue_leaf_creates_ca_and_leaf_files() {
        let tmp = TempDir::new().unwrap();
        let ca_dir = tmp.path().join("ca");
        let cert_path = tmp.path().join("leaf/leaf.pem");
        let key_path = tmp.path().join("leaf/leaf-key.pem");
        let hosts = vec!["localhost".to_string(), "127.0.0.1".to_string()];

        let issued = issue_leaf(&ca_dir, &cert_path, &key_path, &hosts).unwrap();

        let ca = ca_paths(&ca_dir);
        assert!(ca.cert_path.exists(), "CA cert should exist");
        assert!(ca.key_path.exists(), "CA key should exist");
        assert!(issued.cert_path.exists(), "leaf cert should exist");
        assert!(issued.key_path.exists(), "leaf key should exist");
    }

    #[test]
    fn issue_leaf_writes_parseable_pem() {
        let tmp = TempDir::new().unwrap();
        let ca_dir = tmp.path().join("ca");
        let cert_path = tmp.path().join("leaf.pem");
        let key_path = tmp.path().join("leaf-key.pem");
        let hosts = vec!["localhost".to_string()];

        let issued = issue_leaf(&ca_dir, &cert_path, &key_path, &hosts).unwrap();

        let cert_pem = fs::read_to_string(&issued.cert_path).unwrap();
        assert!(
            cert_pem.contains("BEGIN CERTIFICATE"),
            "leaf cert PEM should contain a certificate header"
        );

        let key_pem = fs::read_to_string(&issued.key_path).unwrap();
        rcgen::KeyPair::from_pem(&key_pem).expect("leaf key PEM should round-trip");
    }

    #[cfg(unix)]
    #[test]
    fn issue_leaf_sets_unix_key_permissions() {
        let tmp = TempDir::new().unwrap();
        let ca_dir = tmp.path().join("ca");
        let cert_path = tmp.path().join("leaf.pem");
        let key_path = tmp.path().join("leaf-key.pem");
        let hosts = vec!["localhost".to_string()];

        let issued = issue_leaf(&ca_dir, &cert_path, &key_path, &hosts).unwrap();

        assert_eq!(mode_of(&issued.key_path), 0o600, "leaf key should be 0o600");
    }

    #[test]
    fn validity_window_brackets_now_and_spans_expected_seconds() {
        let valid_days = 397;
        let (not_before, not_after) = validity_window(valid_days).unwrap();
        let now = OffsetDateTime::now_utc();

        assert!(not_before < now, "not_before should precede now");
        assert!(now < not_after, "not_after should follow now");

        let span = (not_after - not_before).whole_seconds();
        let expected = DAY_SECONDS * (valid_days + 1);
        assert_eq!(
            span, expected,
            "validity window should span (valid_days + 1) days of seconds"
        );
    }
}
