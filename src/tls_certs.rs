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
pub(crate) struct CertKeyPaths {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct CaPaths {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

pub(crate) fn ca_paths(dir: &Path) -> CaPaths {
    CaPaths {
        cert_path: dir.join("phoenix-local-ca.pem"),
        key_path: dir.join("phoenix-local-ca-key.pem"),
    }
}

pub(crate) fn ensure_ca(dir: &Path) -> Result<CaPaths, Box<dyn Error>> {
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

pub(crate) fn issue_leaf(
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
    fs::write(path, contents)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = match kind {
            PemKind::Public => 0o644,
            PemKind::Private => 0o600,
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }

    Ok(())
}
