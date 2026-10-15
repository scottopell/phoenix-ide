#![cfg(target_os = "macos")]

use std::{path::PathBuf, process::Command};

use phoenix_deployment_controller::{
    ControllerOs, IdentityObservation, MacOsControllerOs, ResourceObservationFailure, Signal,
    SignalOutcome, SupervisionRisk,
};

#[test]
fn supervised_child_has_stable_identity_and_bounded_group_cleanup() {
    let mut os = MacOsControllerOs::new(PathBuf::from("/tmp"))
        .expect("native resource baseline is observable");
    let mut command = Command::new("/bin/sleep");
    command.arg("30");
    let mut child = os
        .spawn_supervised(&mut command)
        .expect("isolated harmless child starts in its own process group");

    assert!(matches!(
        os.observe_identity(child.identity()),
        IdentityObservation::SameIdentity { .. }
    ));
    assert!(os
        .list_children(child.identity())
        .expect("owned child enumeration succeeds")
        .is_empty());
    assert_eq!(child.process_group(), child.identity().pid());
    assert_eq!(
        MacOsControllerOs::supervision_risks(),
        [
            SupervisionRisk::DescendantMayEscapeProcessGroup,
            SupervisionRisk::SameSessionProcessMayJoinProcessGroup,
            SupervisionRisk::ControllerExitLosesParentAuthority,
        ]
    );

    assert_eq!(
        child.signal_group(Signal::Terminate),
        SignalOutcome::Delivered
    );
    let status = child.wait().expect("owned child is reaped");
    assert!(!status.success());
}

#[test]
fn native_resource_sample_reports_capacity_and_measured_contention() {
    let mut os = MacOsControllerOs::new(PathBuf::from("/tmp"))
        .expect("native resource baseline is observable");
    let mut observed = None;
    for sequence in 0..10_000 {
        std::hint::spin_loop();
        match os.sample_resources(sequence) {
            Ok(sample) => {
                observed = Some(sample);
                break;
            }
            Err(ResourceObservationFailure::TemporarilyUnavailable) => {}
            Err(error) => panic!("native resource observation failed: {error:?}"),
        }
    }
    let sample = observed.expect("CPU counters advance within the bounded observation loop");
    assert!(sample.capacity.available_memory_bytes > 0);
    assert!(sample.capacity.available_storage_bytes > 0);
    assert!(sample.contention.cpu.get() <= 10_000);
}
