//! Cheap server-local observations; no database access or recursive sizing.
//! REQ-DEPLOY-005A, REQ-DEPLOY-006A.

use super::deployment::{absolutize, DeploymentConfig, DiskCategory};
use super::AppState;
use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use ts_rs::TS;

const LOW_SPACE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DeploymentObservations {
    pub observed_at: DateTime<Utc>,
    pub low_space_warning_bytes: u64,
    pub filesystems: Vec<FilesystemSpaceObservation>,
    pub measurement_errors: Vec<DiskSpaceMeasurementError>,
    pub failures: Vec<crate::operational_observations::FailureObservation>,
    pub local_authority_failure_boundary: Option<&'static str>,
    pub sinks: crate::logging::observations::DiagnosticSinkObservation,
}

#[derive(Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct SpaceLocation {
    pub label: String,
    pub path: String,
    pub sampled_path: String,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct FilesystemSpaceObservation {
    pub locations: Vec<SpaceLocation>,
    pub available_bytes: u64,
    pub low_space: bool,
    pub sampled_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum SpaceMeasurementFailure {
    PermissionDenied,
    NotFound,
    InvalidPath,
    Io,
}

impl From<io::Error> for SpaceMeasurementFailure {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::InvalidInput | io::ErrorKind::NotADirectory => Self::InvalidPath,
            _ => Self::Io,
        }
    }
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DiskSpaceMeasurementError {
    pub label: String,
    pub path: String,
    pub sampled_path: Option<String>,
    pub error: SpaceMeasurementFailure,
}

pub async fn deployment_observations(
    State(state): State<AppState>,
) -> Json<DeploymentObservations> {
    let targets = targets(&state.deployment);
    let (filesystems, measurement_errors) = tokio::task::spawn_blocking(move || {
        sample_space(targets, resolve_filesystem, available_space)
    })
    .await
    .expect("disk observation worker panicked");
    Json(DeploymentObservations {
        observed_at: Utc::now(),
        low_space_warning_bytes: LOW_SPACE_BYTES,
        filesystems,
        measurement_errors,
        failures: crate::operational_observations::snapshot(),
        local_authority_failure_boundary: state.runtime.observed_local_authority_failure_boundary(),
        sinks: crate::logging::observations::snapshot(&state.deployment.log),
    })
}

fn targets(config: &DeploymentConfig) -> Vec<(String, PathBuf)> {
    let mut targets: Vec<_> = config
        .locations
        .iter()
        .filter(|location| {
            matches!(
                location.category,
                DiskCategory::Database | DiskCategory::DataDirectory
            )
        })
        .map(|location| (location.label.clone(), location.path.clone()))
        .collect();
    targets.push(("Temporary directory".into(), std::env::temp_dir()));
    if let Some(path) = &config.log.file {
        targets.push(("Structured log".into(), PathBuf::from(path)));
    }
    if let Some(path) = &config.log.fatal_file {
        targets.push(("Fatal diagnostic".into(), PathBuf::from(path)));
    }
    targets
}

fn resolve_filesystem(path: &Path) -> io::Result<(u64, PathBuf)> {
    use std::os::unix::fs::MetadataExt;
    let mut candidate = absolutize(path);
    loop {
        match std::fs::metadata(&candidate) {
            Ok(metadata) => return Ok((metadata.dev(), candidate)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !candidate.pop() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn available_space(path: &Path) -> io::Result<u64> {
    let stats = nix::sys::statvfs::statvfs(path)
        .map_err(|error| io::Error::from_raw_os_error(error as i32))?;
    // fsblkcnt_t is u32 on macOS and u64 on Linux.
    #[allow(clippy::useless_conversion)]
    let blocks = u64::from(stats.blocks_available());
    blocks
        .checked_mul(stats.fragment_size())
        .ok_or_else(|| io::Error::other("filesystem byte count overflow"))
}

fn sample_space(
    targets: Vec<(String, PathBuf)>,
    resolve: impl Fn(&Path) -> io::Result<(u64, PathBuf)>,
    available: impl Fn(&Path) -> io::Result<u64>,
) -> (
    Vec<FilesystemSpaceObservation>,
    Vec<DiskSpaceMeasurementError>,
) {
    let mut groups: BTreeMap<u64, FilesystemSpaceObservation> = BTreeMap::new();
    let mut errors = Vec::new();
    for (label, path) in targets {
        let path = absolutize(&path);
        let resolved = resolve(&path);
        let path = path.to_string_lossy().into_owned();
        match resolved {
            Err(error) => errors.push(DiskSpaceMeasurementError {
                label,
                path,
                sampled_path: None,
                error: error.into(),
            }),
            Ok((identity, sampled_path)) => {
                let group = match groups.entry(identity) {
                    std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        match available(&sampled_path) {
                            Ok(available_bytes) => entry.insert(FilesystemSpaceObservation {
                                locations: Vec::new(),
                                available_bytes,
                                low_space: available_bytes < LOW_SPACE_BYTES,
                                sampled_at: Utc::now(),
                            }),
                            Err(error) => {
                                errors.push(DiskSpaceMeasurementError {
                                    label,
                                    path,
                                    sampled_path: Some(sampled_path.to_string_lossy().into_owned()),
                                    error: error.into(),
                                });
                                continue;
                            }
                        }
                    }
                };
                let sampled_path = sampled_path.to_string_lossy().into_owned();
                group.locations.push(SpaceLocation {
                    label,
                    path,
                    sampled_path,
                });
            }
        }
    }
    (groups.into_values().collect(), errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn space_sampling_deduplicates_filesystems_and_preserves_locations_and_errors() {
        let measurements = Cell::new(0);
        let (groups, errors) = sample_space(
            vec![
                ("db".into(), "/db".into()),
                ("data".into(), "/data".into()),
                ("tmp".into(), "/tmp".into()),
                ("log".into(), "/log".into()),
            ],
            |path| {
                if path == Path::new("/log") {
                    Err(io::ErrorKind::PermissionDenied.into())
                } else {
                    Ok((
                        if path == Path::new("/tmp") { 2 } else { 1 },
                        path.to_path_buf(),
                    ))
                }
            },
            |path| {
                measurements.set(measurements.get() + 1);
                Ok(if path == Path::new("/tmp") {
                    LOW_SPACE_BYTES
                } else {
                    0
                })
            },
        );
        assert_eq!(measurements.get(), 2);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].locations.len(), 2);
        assert_eq!(groups[0].available_bytes, 0);
        assert!(groups[0].low_space);
        assert!(!groups[1].low_space);
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0].error,
            SpaceMeasurementFailure::PermissionDenied
        ));
    }

    #[test]
    fn failed_measurement_is_explicit_for_each_location_and_never_zero() {
        let (groups, errors) = sample_space(
            vec![("db".into(), "/db".into()), ("log".into(), "/log".into())],
            |path| Ok((1, path.into())),
            |_| Err(io::ErrorKind::PermissionDenied.into()),
        );
        assert!(groups.is_empty());
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().all(|error| error.sampled_path.is_some()));
    }

    #[test]
    fn missing_target_reports_its_actual_sampled_ancestor() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("not-created/db");
        let (_, resolved) = resolve_filesystem(&target).unwrap();
        assert_eq!(resolved, directory.path());
        assert!(available_space(&resolved).is_ok());
    }

    #[test]
    fn a_failed_location_does_not_hide_a_successful_sample_on_the_same_filesystem() {
        let (groups, errors) = sample_space(
            vec![("db".into(), "/db".into()), ("log".into(), "/log".into())],
            |path| Ok((1, path.into())),
            |path| {
                if path == Path::new("/db") {
                    Err(io::ErrorKind::PermissionDenied.into())
                } else {
                    Ok(LOW_SPACE_BYTES)
                }
            },
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].locations[0].label, "log");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].sampled_path.as_deref(), Some("/db"));
    }
}
