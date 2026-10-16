#![cfg(unix)]
use phoenix_deployment_controller::{
    guardian_ipc::{
        read_frame, write_frame, GuardianServer, Request, Response, WireError, WireSnapshot,
        WireState,
    },
    native_guardian::{bind_disposable, disposable_dir, same_uid, serve_until, GuardianClient},
    AttemptId, ChildSlotId, ChildSlotObservation, DispatchTicket, ExecutedEffect, Guardian,
    GuardianExecutor, MonotonicTick,
};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, io::AsRawFd, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

fn readable_within(stream: &UnixStream, millis: i32) -> bool {
    let mut descriptor = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor is a valid stack value and stream owns the live fd.
    let result = unsafe { libc::poll(&raw mut descriptor, 1, millis) };
    assert!(
        result >= 0,
        "poll failed: {}",
        std::io::Error::last_os_error()
    );
    result > 0
}

struct NoEffects;
impl GuardianExecutor for NoEffects {
    type ChildSlot = ();
    fn execute(
        &mut self,
        _: DispatchTicket,
        _: &mut std::collections::BTreeMap<ChildSlotId, ()>,
    ) -> ExecutedEffect {
        panic!("unbound transport cannot execute")
    }
    fn poll_observation_only(&self, (): &()) -> ChildSlotObservation {
        ChildSlotObservation::Running
    }
}

fn exercise_protocol(directory: &Path, hello: WireSnapshot) {
    let mut raw = UnixStream::connect(directory.join("guardian.sock")).unwrap();
    same_uid(&raw).unwrap();
    let mut hello_frame = Vec::new();
    write_frame(&mut hello_frame, &Request::Hello).unwrap();
    raw.write_all(&hello_frame[..2]).unwrap();
    assert!(!readable_within(&raw, 20));
    raw.write_all(&hello_frame[2..5]).unwrap();
    assert!(!readable_within(&raw, 20));
    raw.write_all(&hello_frame[5..]).unwrap();
    let Response::Hello { snapshot } = read_frame(&mut raw).unwrap() else {
        panic!("hello")
    };
    assert_eq!(snapshot.instance, hello.instance);
    assert_eq!(snapshot.state, WireState::DetachedFenced);
    write_frame(
        &mut raw,
        &Request::BeginReconciliation {
            instance: hello.instance,
            inventory: hello.slots,
            interruption_revision: 1,
        },
    )
    .unwrap();
    assert_eq!(
        read_frame::<Response>(&mut raw).unwrap(),
        Response::Refused(WireError::UnsupportedDurableBinding)
    );
    raw.write_all(&0_u32.to_be_bytes()).unwrap();
    drop(raw);
    let mut stalled = UnixStream::connect(directory.join("guardian.sock")).unwrap();
    stalled.write_all(&hello_frame[..2]).unwrap();
    assert!(readable_within(&stalled, 1000));
    assert_eq!(std::io::Read::read(&mut stalled, &mut [0; 1]).unwrap(), 0);
    drop(stalled);
    let mut oversized = UnixStream::connect(directory.join("guardian.sock")).unwrap();
    oversized
        .write_all(
            &u32::try_from(phoenix_deployment_controller::guardian_ipc::MAX_FRAME_BYTES + 1)
                .unwrap()
                .to_be_bytes(),
        )
        .unwrap();
    drop(oversized);
    let mut replacement = GuardianClient::connect(directory).unwrap();
    assert_eq!(
        replacement.hello().unwrap().state,
        WireState::DetachedFenced
    );
}

#[test]
fn private_socket_fences_on_eof_and_protocol_failure_but_accepts_replacement() {
    let parent = tempfile::Builder::new()
        .prefix("sdc-")
        .tempdir_in("/tmp")
        .unwrap();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let directory = parent.path().join("sdc-disposable-socket");
    let listener = bind_disposable(&directory).unwrap();
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(directory.join("guardian.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let (guardian, session) = Guardian::new(
        AttemptId::new(1),
        MonotonicTick::new(200),
        NoEffects,
        vec![()],
    )
    .unwrap();
    let mut server = GuardianServer::new(guardian, session).unwrap();
    server.require_native_binding();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            serve_until(
                &listener,
                &mut server,
                Instant::now() + Duration::from_secs(3),
            )
            .unwrap();
        });
        let mut client = GuardianClient::connect(&directory).unwrap();
        let hello = client.hello().unwrap();
        assert_eq!(hello.slots.len(), 1);
        assert_eq!(client.inventory().unwrap(), hello.slots);
        assert_eq!(
            client.observe_slot(hello.slots[0]).unwrap(),
            phoenix_deployment_controller::guardian_ipc::WireObservation::Running
        );
        assert!(matches!(
            client.reconcile(),
            Err(
                phoenix_deployment_controller::native_guardian::ClientError::Refused(
                    WireError::UnsupportedDurableBinding
                )
            )
        ));
        drop(client);
        exercise_protocol(&directory, hello);
    });
}

#[test]
fn disposable_path_rejects_existing_symlink_and_broad_parent() {
    let parent = tempfile::Builder::new()
        .prefix("sdc-")
        .tempdir_in("/tmp")
        .unwrap();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let private = parent.path().join("sdc-disposable-safe");
    bind_disposable(&private).unwrap();
    assert!(bind_disposable(&private).is_err());
    assert!(disposable_dir(
        std::env::temp_dir().join("sdc-disposable-unsafe").as_path(),
        true
    )
    .is_err());
    let link = parent.path().join("sdc-disposable-link");
    std::os::unix::fs::symlink(&private, &link).unwrap();
    assert!(disposable_dir(&link, false).is_err());
    assert!(disposable_dir(parent.path().join("phoenix-production").as_path(), true).is_err());
}
