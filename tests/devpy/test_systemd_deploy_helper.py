import dataclasses
import hashlib
import importlib.util
import json
import os
import pwd
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_helper():
    path = ROOT / "scripts/systemd_deploy_helper.py"
    spec = importlib.util.spec_from_file_location("systemd_deploy_helper_test", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


helper = load_helper()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class InstalledHealthUrlTests(unittest.TestCase):
    def test_tls_modes_match_controller_and_runtime(self):
        for value, scheme in (("1", "https"), ("true", "https"), ("on", "https"),
                              ("auto", "https"), ("manual", "https"), (" TRUE ", "https"),
                              ("none", "http"), ("disabled", "http"), ("off", "http"),
                              ("false", "http"), ("0", "http"), ("", "http"),
                              (" NONE ", "http"), ("unknown", "http")):
            with self.subTest(value=value):
                self.assertEqual(f"{scheme}://localhost:8031/api/version",
                                 helper.installed_health_url({"PHOENIX_TLS": value}))
        for paths, scheme in (({}, "http"), ({"PHOENIX_TLS_CERT_PATH": "/cert"}, "http"),
                              ({"PHOENIX_TLS_KEY_PATH": "/key"}, "http"),
                              ({"PHOENIX_TLS_CERT_PATH": "/cert", "PHOENIX_TLS_KEY_PATH": "/key"}, "https")):
            with self.subTest(paths=paths):
                self.assertEqual(f"{scheme}://localhost:8123/api/version",
                                 helper.installed_health_url({"PHOENIX_TLS": "none", "PHOENIX_PORT": "8123", **paths}))


class SystemdManifestValidationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.base = Path(self.temporary.name)
        self.transactions = self.base / "transactions"
        self.transaction_id = "a" * 32
        self.transaction = self.transactions / self.transaction_id
        self.transaction.mkdir(parents=True, mode=0o700)
        self.user = pwd.getpwuid(os.getuid()).pw_name
        if os.getuid() == 0:
            self.skipTest("validation tests require a non-root fixture service user")
        self.targets = helper.SystemdTargets(**{
            "binary": str(self.base / "install/phoenix-ide"),
            "service": str(self.base / "units/fixture.service"),
            "socket": str(self.base / "units/fixture.socket"),
            "environment": str(self.base / "config/fixture.env"),
            "deployed_sha": str(self.base / "deployed.sha"),
        })
        self.policy = helper.ValidationPolicy(
            transaction_root=self.transactions,
            unit_name="phoenix-qa-systemd-test",
            targets=self.targets,
            owner_uid=os.getuid(),
            status_path=self.base / "status.json",
            active_path=self.base / "active",
            activation_lock_path=self.base / "activation.lock",
            claim_lock_path=self.base / "claim.lock",
        )
        self.manifest_path = self.transaction / "manifest.json"
        self.raw = self.make_manifest()
        self.write_manifest()

    def tearDown(self):
        self.temporary.cleanup()

    def artifact(self, name, content):
        path = self.transaction / name
        path.write_text(content)
        path.chmod(0o600)
        return {"path": str(path), "sha256": digest(path)}

    def absent(self):
        return {"path": None, "sha256": None}

    def make_manifest(self):
        candidate = {
            "binary": self.artifact("candidate", "binary"),
            "service": self.artifact("candidate.service", "service"),
            "socket": self.artifact("candidate.socket", "socket"),
            "environment": self.artifact("candidate.env", "SECRET=value"),
        }
        return {
            "manifest_version": 1,
            "transaction_id": self.transaction_id,
            "unit_name": self.policy.unit_name,
            "service_user": self.user,
            "source_kind": "local_head",
            "source_commit": "b" * 40,
            "release_tag": None,
            "release_commit": None,
            "expected": {"version": "2.0.0", "git_sha": "b" * 40},
            "previous": None,
            "expected_health_url": "http://127.0.0.1:49152/api/version",
            "previous_health_url": None,
            "candidate": candidate,
            "rollback": {name: self.absent() for name in ("binary", "service", "socket", "environment")},
            "targets": dict(vars(self.targets)),
            "status_path": str(self.base / "status.json"),
            "active_path": str(self.base / "active"),
            "activation_lock_path": str(self.base / "activation.lock"),
            "claim_lock_path": str(self.base / "claim.lock"),
            "previous_deployed_sha": None,
            "created_at": "2026-07-15T00:00:00+00:00",
        }

    def write_manifest(self):
        self.manifest_path.write_text(json.dumps(self.raw))
        self.manifest_path.chmod(0o600)

    def validate(self):
        manifest = helper.Manifest.load(self.manifest_path)
        helper.validate_manifest(self.manifest_path, manifest, self.policy)

    def test_accepts_root_owned_policy_bound_transaction(self):
        self.validate()

    def test_candidate_identity_requires_full_source_commit_equality(self):
        for git_sha in ("b" * 12, "c" * 40, "B" * 40):
            with self.subTest(git_sha=git_sha):
                self.raw["expected"]["git_sha"] = git_sha
                self.write_manifest()
                with self.assertRaisesRegex(helper.ValidationError, "candidate identity"):
                    self.validate()

    def test_previous_identity_accepts_legacy_or_full_sha(self):
        self.raw["previous_health_url"] = "http://127.0.0.1:49151/api/version"
        for git_sha in ("a" * 12, "a" * 40):
            with self.subTest(git_sha=git_sha):
                helper.validate_previous_identity(helper.Identity("1.0.0", git_sha))
        for git_sha in ("a" * 11, "a" * 13, "A" * 40):
            with self.subTest(git_sha=git_sha):
                with self.assertRaisesRegex(helper.ValidationError, "previous identity"):
                    helper.validate_previous_identity(helper.Identity("1.0.0", git_sha))

    def test_previous_deployed_sha_remains_full_length(self):
        self.raw["previous_deployed_sha"] = "a" * 12
        self.write_manifest()
        with self.assertRaisesRegex(helper.ValidationError, "previous deployed SHA"):
            self.validate()

    def test_rejects_protocol_mismatch(self):
        self.raw["manifest_version"] = 99
        self.write_manifest()
        with self.assertRaisesRegex(helper.ValidationError, "unsupported handoff protocol"):
            self.validate()

    def test_rejects_arbitrary_target(self):
        self.raw["targets"]["binary"] = "/usr/local/bin/unrelated"
        self.write_manifest()
        with self.assertRaisesRegex(helper.ValidationError, "target paths are not allowed"):
            self.validate()

    def test_rejects_malformed_source_metadata(self):
        self.raw["source_kind"] = "remote_branch"
        self.write_manifest()
        with self.assertRaisesRegex(helper.ValidationError, "source kind is not allowed"):
            self.validate()

    def test_rejects_non_loopback_health_endpoint(self):
        self.raw["expected_health_url"] = "https://example.com/api/version?token=secret"
        self.write_manifest()
        with self.assertRaisesRegex(helper.ValidationError, "loopback HTTP endpoint"):
            self.validate()

    def test_rejects_arbitrary_status_path(self):
        self.raw["status_path"] = str(self.base / "other-status.json")
        self.write_manifest()
        with self.assertRaisesRegex(helper.ValidationError, "status path is not allowed"):
            self.validate()

    def test_rejects_tampered_artifact(self):
        Path(self.raw["candidate"]["binary"]["path"]).write_text("tampered")
        with self.assertRaisesRegex(helper.ValidationError, "checksum mismatch"):
            self.validate()

    def test_rejects_symlink_artifact(self):
        original = Path(self.raw["candidate"]["service"]["path"])
        actual = self.transaction / "actual.service"
        actual.write_text(original.read_text())
        original.unlink()
        original.symlink_to(actual)
        with self.assertRaisesRegex(helper.ValidationError, "cannot be opened safely"):
            self.validate()

    def test_rejects_world_readable_environment(self):
        Path(self.raw["candidate"]["environment"]["path"]).chmod(0o644)
        with self.assertRaisesRegex(helper.ValidationError, "permissions are too broad"):
            self.validate()

    def test_rejects_wrong_unit_name(self):
        self.raw["unit_name"] = "phoenix-ide"
        self.write_manifest()
        with self.assertRaisesRegex(helper.ValidationError, "unit name is not allowed"):
            self.validate()

    def test_rejects_root_service_user(self):
        self.raw["service_user"] = "root"
        self.write_manifest()
        with self.assertRaisesRegex(helper.ValidationError, "must not be root"):
            self.validate()


class SystemdHandoffStagingTests(SystemdManifestValidationTests):
    def bundle(self, files):
        path = self.base / "bundle.json"
        path.write_text(json.dumps({"transaction_id": self.transaction_id, "files": files}))
        return path

    def staging_policy(self):
        staging = self.base / "root-deploy"
        return helper.ValidationPolicy(
            transaction_root=staging / "transactions",
            unit_name=self.policy.unit_name,
            targets=self.targets,
            status_path=staging / "status.json",
            active_path=staging / "active",
            activation_lock_path=staging / "activation.lock",
            claim_lock_path=staging / "claim.lock",
            owner_uid=os.getuid(),
        )

    def staging_sources(self):
        sources = []
        for name in ("candidate-binary", "candidate.service", "candidate.socket", "helper.py"):
            source = self.base / f"source-{name}"
            source.write_text(name)
            sources.append({"name": name, "source": str(source), "sha256": digest(source)})
        return sources

    def test_stage_handoff_copies_allowlisted_files_and_acquires_claim(self):
        sources = self.staging_sources()
        source_manifest = self.base / "source-manifest.json"
        source_manifest.write_text(json.dumps(self.raw))
        sources.append({"name": "manifest.json", "source": str(source_manifest), "sha256": digest(source_manifest)})
        policy = self.staging_policy()
        with mock.patch.object(helper, "prepare_data_directory", return_value=True):
            manifest = helper.stage_handoff(self.bundle(sources), os.getuid(), policy)
        self.assertEqual(policy.transaction_root / self.transaction_id / "manifest.json", manifest)
        self.assertEqual(self.transaction_id, policy.active_path.read_text().strip())
        self.assertEqual("candidate-binary", (manifest.parent / "candidate-binary").read_text())
        self.assertEqual(self.raw["expected"], json.loads(manifest.read_text())["expected"])
        status = json.loads(policy.status_path.read_text())
        self.assertEqual("prepared", status["state"])
        self.assertEqual(self.transaction_id, status["transaction_id"])

    def test_root_staging_derives_previous_runtime_from_captured_install(self):
        sources = self.staging_sources()
        source_manifest = self.base / "manifest-source.json"
        stale = dict(self.raw)
        stale["previous"] = None
        stale["previous_health_url"] = None
        source_manifest.write_text(json.dumps(stale))
        sources.append({"name": "manifest.json", "source": str(source_manifest), "sha256": digest(source_manifest)})
        policy = self.staging_policy()
        Path(policy.targets.binary).parent.mkdir(parents=True, exist_ok=True)
        Path(policy.targets.service).parent.mkdir(parents=True, exist_ok=True)
        Path(policy.targets.binary).write_text("installed binary")
        Path(policy.targets.service).write_text("[Service]\nUser=nobody\n")
        Path(policy.targets.socket).write_text("[Socket]\nListenStream=9443\n")
        Path(policy.targets.environment).parent.mkdir(parents=True, exist_ok=True)
        Path(policy.targets.environment).write_text("PHOENIX_PORT=9443\nPHOENIX_TLS=auto\n")
        identity = helper.Identity(version="1.2.3", git_sha="a" * 12)
        original_copy = helper.copy_handoff_file

        def copy_file(source, destination, expected_sha, source_uid, mode):
            if source in map(Path, dataclasses.astuple(policy.targets)[:4]):
                shutil.copy2(source, destination)
                destination.chmod(mode)
            else:
                original_copy(source, destination, expected_sha, source_uid, mode)

        with mock.patch.object(helper, "prepare_data_directory", return_value=True), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=identity), \
             mock.patch.object(helper, "copy_handoff_file", side_effect=copy_file):
            manifest_path = helper.stage_handoff(self.bundle(sources), os.getuid(), policy)
        manifest = helper.Manifest.load(manifest_path)
        self.assertEqual(identity, manifest.previous)
        self.assertEqual("https://localhost:9443/api/version", manifest.previous_health_url)
        self.assertIsNotNone(manifest.rollback.binary.path)

    def test_stage_handoff_rejects_non_allowlisted_artifact_before_claim(self):
        source = self.base / "secret"
        source.write_text("secret")
        policy = self.staging_policy()
        with self.assertRaisesRegex(helper.ValidationError, "non-allowlisted"):
            helper.stage_handoff(
                self.bundle([{"name": "arbitrary", "source": str(source), "sha256": digest(source)}]),
                os.getuid(),
                policy,
            )
        self.assertFalse(policy.active_path.exists())

    def test_stage_handoff_failure_releases_only_its_own_claim(self):
        source = self.base / "candidate"
        source.write_text("candidate")
        required = [
            {"name": name, "source": str(source), "sha256": "0" * 64}
            for name in ("candidate-binary", "candidate.service", "candidate.socket", "helper.py", "manifest.json")
        ]
        policy = self.staging_policy()
        with self.assertRaisesRegex(helper.ValidationError, "checksum mismatch"):
            helper.stage_handoff(self.bundle(required), os.getuid(), policy)
        self.assertFalse(policy.active_path.exists())
        self.assertFalse((policy.transaction_root / self.transaction_id).exists())

    def test_prepared_status_sync_exception_preserves_evidence_and_claim(self):
        policy = self.staging_policy()
        sources = self.staging_sources()
        manifest = self.base / "source-manifest.json"
        manifest.write_text(json.dumps(self.raw))
        sources.append({"name": "manifest.json", "source": str(manifest), "sha256": digest(manifest)})
        original = helper.write_policy_status
        def uncertain(*args, **kwargs):
            original(*args, **kwargs)
            raise OSError("prepared directory sync uncertain")
        with mock.patch.object(helper, "prepare_data_directory", return_value=True), \
             mock.patch.object(helper, "write_policy_status", side_effect=uncertain):
            with self.assertRaises(OSError):
                helper.stage_handoff(self.bundle(sources), os.getuid(), policy)
        self.assertTrue((policy.transaction_root / self.transaction_id / "manifest.json").exists())
        self.assertEqual(self.transaction_id, policy.active_path.read_text().strip())
        self.assertEqual("prepared", json.loads(policy.status_path.read_text())["state"])
        with self.assertRaises(helper.ConcurrentDeploy):
            helper.stage_handoff(self.bundle(sources), os.getuid(), policy)

    def test_new_staging_claim_retires_only_exact_partial_transaction(self):
        policy = self.staging_policy()
        stale = helper.acquire_claim(self.transaction_id, policy)
        (stale / "partial").write_text("partial input")
        unrelated = policy.transaction_root / ("f" * 32)
        unrelated.mkdir()
        (unrelated / "keep").write_text("unrelated evidence")
        with policy.activation_lock_path.open("a+") as lock:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX)
            new = helper.acquire_claim("e" * 32, policy, retire_staging=True)
        self.assertFalse(stale.exists())
        self.assertEqual("e" * 32, new.name)
        self.assertEqual("e" * 32, policy.active_path.read_text().strip())
        self.assertEqual("unrelated evidence", (unrelated / "keep").read_text())

    def test_stage_holds_activation_lock_through_copy_and_prepared_status(self):
        import fcntl
        policy = self.staging_policy()
        sources = self.staging_sources()
        manifest = self.base / "source-manifest.json"
        manifest.write_text(json.dumps(self.raw))
        sources.append({"name": "manifest.json", "source": str(manifest), "sha256": digest(manifest)})
        original_copy, original_status = helper.copy_handoff_file, helper.write_policy_status
        def locked():
            with policy.activation_lock_path.open("a+") as lock:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        def copy(*args, **kwargs):
            locked()
            return original_copy(*args, **kwargs)
        def status(*args, **kwargs):
            locked()
            return original_status(*args, **kwargs)
        with mock.patch.object(helper, "copy_handoff_file", side_effect=copy), \
             mock.patch.object(helper, "write_policy_status", side_effect=status), \
             mock.patch.object(helper, "prepare_data_directory", return_value=True):
            helper.stage_handoff(self.bundle(sources), os.getuid(), policy)

    def test_staging_process_crash_boundaries_reenter_without_orphan_claim(self):
        sources = self.staging_sources()
        manifest = self.base / "source-manifest.json"
        manifest.write_text(json.dumps(self.raw))
        sources.append({"name": "manifest.json", "source": str(manifest), "sha256": digest(manifest)})
        bundle = self.bundle(sources)
        policy = self.staging_policy()
        for boundary in ("before_status", "after_status", "after_claim", "first_copy", "manifest_copy", "prepared"):
            with self.subTest(boundary=boundary):
                shutil.rmtree(policy.transaction_root.parent, ignore_errors=True)
                pid = os.fork()
                if pid == 0:
                    original_write, original_copy = helper.atomic_write, helper.copy_handoff_file
                    def write(path, *args, **kwargs):
                        if path == policy.status_path and boundary == "before_status":
                            os._exit(77)
                        original_write(path, *args, **kwargs)
                        if path == policy.status_path:
                            state = json.loads(path.read_text())["state"]
                            if (boundary == "after_status" and state == "staging") or (boundary == "prepared" and state == "prepared"):
                                os._exit(77)
                        if path == policy.active_path and boundary == "after_claim":
                            os._exit(77)
                    def copy(source, destination, *args):
                        original_copy(source, destination, *args)
                        if (boundary == "first_copy" and destination.name == "candidate-binary") or (boundary == "manifest_copy" and destination.name == "manifest.json"):
                            os._exit(77)
                    try:
                        with mock.patch.object(helper, "atomic_write", side_effect=write), \
                             mock.patch.object(helper, "copy_handoff_file", side_effect=copy), \
                             mock.patch.object(helper, "prepare_data_directory", return_value=True):
                            helper.stage_handoff(bundle, os.getuid(), policy)
                    finally:
                        os._exit(78)
                _, status = os.waitpid(pid, 0)
                self.assertEqual(77, os.waitstatus_to_exitcode(status))
                if policy.active_path.exists():
                    self.assertEqual(self.transaction_id, json.loads(policy.status_path.read_text())["transaction_id"])
                with mock.patch.object(helper, "prepare_data_directory", return_value=True):
                    if boundary == "prepared":
                        before = policy.status_path.read_bytes()
                        with self.assertRaises(helper.ConcurrentDeploy):
                            helper.stage_handoff(bundle, os.getuid(), policy)
                        self.assertEqual(before, policy.status_path.read_bytes())
                    else:
                        helper.stage_handoff(bundle, os.getuid(), policy)
                        self.assertEqual("prepared", json.loads(policy.status_path.read_text())["state"])

    def test_staging_retirement_refuses_marker_mismatch_or_ambiguous_status(self):
        policy = self.staging_policy()
        policy.transaction_root.mkdir(parents=True)
        transaction = policy.transaction_root / self.transaction_id
        transaction.mkdir()
        unrelated = policy.transaction_root / ("f" * 32)
        unrelated.mkdir()
        (unrelated / "keep").write_text("evidence")
        original = {"transaction_id": self.transaction_id, "state": "staging", "migration_started": False}
        policy.active_path.write_text(self.transaction_id + "\n")
        for changes in ({"migration_started": True}, {"state": "activating"}, {"extra": "ambiguous"}, {"transaction_id": "f" * 32}):
            policy.status_path.write_text(json.dumps({**original, **changes}))
            with self.assertRaises(helper.ConcurrentDeploy):
                helper.acquire_claim("e" * 32, policy, retire_staging=True)
            self.assertTrue(transaction.exists())
        policy.status_path.write_text(json.dumps(original))
        (transaction / "migration_started").write_text("irreversible")
        with self.assertRaises(helper.ConcurrentDeploy):
            helper.acquire_claim("e" * 32, policy, retire_staging=True)
        self.assertEqual("evidence", (unrelated / "keep").read_text())
        policy.active_path.unlink()
        for state in ("prepared", "activating", "committed"):
            policy.status_path.write_text(json.dumps({**original, "state": state}))
            with self.subTest(state=state), self.assertRaises(helper.ConcurrentDeploy):
                helper.acquire_claim("e" * 32, policy, retire_staging=True)
            self.assertTrue((transaction / "migration_started").exists())


class FakeSystemctl:
    def __init__(self, manifest, previous_state, *, start_failure=None):
        self.manifest = manifest
        self.service = f"{manifest.unit_name}.service"
        self.socket = f"{manifest.unit_name}.socket"
        self.previous_state = previous_state
        self.disruption_started = False
        self.start_failure = start_failure
        self.events = []

    def verify_units(self, service, socket, candidate_binary):
        self.events.append("verify")

    def inspect(self):
        self.events.append("inspect")
        return self.previous_state

    def stop(self):
        self.events.append("stop")
        self.disruption_started = True

    def command(self, *args, check=True):
        self.events.append(("command", args, check))
        return subprocess.CompletedProcess(args, 0, "", "")

    def daemon_reload(self):
        self.events.append("reload")

    def start_candidate(self, old_pid):
        self.events.append(("start_candidate", old_pid))
        if self.start_failure:
            raise helper.ActivationError(self.start_failure)
        return old_pid + 1

    def restore_state(self, previous):
        self.events.append(("restore_state", previous))
        return 99


class SystemdActivationTests(SystemdManifestValidationTests):
    def install_previous(self):
        for target, content in (
            (self.targets.binary, "old binary"),
            (self.targets.service, f"[Service]\nUser={pwd.getpwuid(os.getuid()).pw_name}\n"),
            (self.targets.socket, "old socket"),
            (self.targets.environment, "OLD=value"),
        ):
            path = Path(target)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        Path(self.targets.deployed_sha).write_text("a" * 40 + "\n")
        self.raw["previous"] = {"version": "1.0.0", "git_sha": "a" * 12}
        self.raw["previous_health_url"] = "http://127.0.0.1:49151/api/version"
        self.raw["previous_deployed_sha"] = "a" * 40
        for name, target in (
            ("binary", self.targets.binary),
            ("service", self.targets.service),
            ("socket", self.targets.socket),
            ("environment", self.targets.environment),
        ):
            self.raw["rollback"][name] = self.artifact(f"rollback-{name}", Path(target).read_text())
        self.write_manifest()

    def manifest(self):
        return helper.Manifest.load(self.manifest_path)

    def test_activation_creates_lock_parent_before_opening_lock(self):
        manifest = self.manifest()
        lock = Path(manifest.activation_lock_path)
        shutil.rmtree(lock.parent)
        controller = FakeSystemctl(manifest, helper.UnitState(False, False, False, False, 0))
        with mock.patch.object(helper, "database_preflight", return_value=None), \
             mock.patch.object(helper, "prepare_installs", side_effect=helper.ActivationError("stop after lock")):
            with self.assertRaisesRegex(helper.ActivationError, "stop after lock"):
                helper.activate(manifest, controller)
        self.assertTrue(lock.parent.is_dir())

    def test_unit_verification_uses_staged_candidate_executable(self):
        manifest = self.manifest()
        calls = []
        service = Path(manifest.candidate.service.path)
        service.write_text(f"[Service]\nExecStart={manifest.targets.binary}\n")

        def run(command, **_kwargs):
            calls.append(command)
            verification_service = Path(command[-1])
            self.assertEqual(f"{manifest.unit_name}.service", verification_service.name)
            self.assertEqual(f"{manifest.unit_name}.socket", Path(command[-2]).name)
            self.assertIn(manifest.candidate.binary.path, verification_service.read_text())
            self.assertNotIn(manifest.targets.binary, verification_service.read_text())
            return subprocess.CompletedProcess(command, 0, "", "")

        helper.Systemctl(manifest, run=run).verify_units(
            Path(manifest.candidate.service.path),
            Path(manifest.candidate.socket.path),
            Path(manifest.candidate.binary.path),
        )
        self.assertEqual("systemd-analyze", calls[0][0])

    def test_legacy_dropins_block_activation_before_disruption(self):
        manifest = self.manifest()
        controller = FakeSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        with tempfile.TemporaryDirectory() as td:
            override_dir = Path(td)
            (override_dir / "legacy.conf").write_text("[Service]\nEnvironment=X=1\n")
            with mock.patch.object(helper, "legacy_override_dir", return_value=override_dir):
                with self.assertRaisesRegex(helper.ActivationError, "move required values to .phoenix-ide.env"):
                    helper.prepare_installs(manifest, controller)
        self.assertFalse(controller.disruption_started)

    def test_success_atomically_installs_candidate_and_commits(self):
        self.install_previous()
        manifest = self.manifest()
        previous_state = helper.UnitState(True, True, True, True, 41)
        controller = FakeSystemctl(manifest, previous_state)
        with mock.patch.object(helper, "wait_for_identity") as verify:
            state = helper.activate(manifest, controller)
        self.assertEqual("committed", state)
        self.assertEqual("binary", Path(self.targets.binary).read_text())
        self.assertEqual("service", Path(self.targets.service).read_text())
        self.assertEqual("socket", Path(self.targets.socket).read_text())
        self.assertEqual("SECRET=value", Path(self.targets.environment).read_text())
        self.assertEqual(pwd.getpwnam(manifest.service_user).pw_gid, Path(self.targets.environment).stat().st_gid)
        self.assertEqual(0o640, Path(self.targets.environment).stat().st_mode & 0o777)
        self.assertEqual("b" * 40, Path(self.targets.deployed_sha).read_text().strip())
        self.assertEqual("committed", json.loads(self.policy.status_path.read_text())["state"])
        verify.assert_called_once_with(manifest, manifest.expected, manifest.expected_health_url)
        self.assertEqual(["verify", "inspect", "stop", "reload", ("start_candidate", 41)], controller.events)

    def test_identity_failure_restores_previous_artifacts_and_state(self):
        self.install_previous()
        manifest = self.manifest()
        previous_state = helper.UnitState(True, False, True, True, 41)
        controller = FakeSystemctl(manifest, previous_state)
        calls = iter([helper.ActivationError("wrong identity"), None])

        def verify(*_args):
            outcome = next(calls)
            if outcome:
                raise outcome

        with mock.patch.object(helper, "wait_for_identity", side_effect=verify):
            state = helper.activate(manifest, controller)
        self.assertEqual("activation_failed_rolled_back", state)
        self.assertEqual("old binary", Path(self.targets.binary).read_text())
        self.assertEqual(
            f"[Service]\nUser={pwd.getpwuid(os.getuid()).pw_name}\n",
            Path(self.targets.service).read_text(),
        )
        self.assertEqual("old socket", Path(self.targets.socket).read_text())
        self.assertEqual("OLD=value", Path(self.targets.environment).read_text())
        self.assertEqual("a" * 40, Path(self.targets.deployed_sha).read_text().strip())
        status = json.loads(self.policy.status_path.read_text())
        self.assertEqual("activation_failed_rolled_back", status["state"])
        self.assertEqual("wrong identity", status["failure"])
        self.assertIn(("restore_state", previous_state), controller.events)

    def test_rollback_restores_previous_data_directory_owner(self):
        self.install_previous()
        manifest = self.manifest()
        controller = FakeSystemctl(manifest, helper.UnitState(True, False, True, True, 41))
        calls = iter([helper.ActivationError("wrong identity"), None])

        def verify(*_args):
            outcome = next(calls)
            if outcome:
                raise outcome

        with mock.patch.object(helper, "wait_for_identity", side_effect=verify), \
             mock.patch.object(helper, "prepare_data_directory") as ownership:
            helper.activate(manifest, controller)
        ownership.assert_called_once_with(
            pwd.getpwuid(os.getuid()).pw_name,
            manifest.targets.deployed_sha,
        )

    def test_stopped_previous_service_skips_rollback_health_probe(self):
        self.install_previous()
        manifest = self.manifest()
        controller = FakeSystemctl(manifest, helper.UnitState(False, False, False, False, 0))
        with mock.patch.object(
            helper,
            "wait_for_identity",
            side_effect=[helper.ActivationError("candidate failed")],
        ) as verify, mock.patch.object(helper, "prepare_data_directory"):
            state = helper.activate(manifest, controller)
        self.assertEqual("activation_failed_rolled_back", state)
        self.assertEqual(1, verify.call_count)

    def test_rollback_environment_uses_previous_service_group(self):
        self.install_previous()
        manifest = self.manifest()
        previous_user = pwd.getpwuid(os.getuid()).pw_name
        candidate_gid = 111
        previous_gid = 222
        owners = []

        def group_id(user):
            return previous_gid if user == previous_user else candidate_gid

        def reserve(_staged, target, _mode, *, owner_uid=None, owner_gid=None):
            owners.append((target, owner_gid))
            reserved = target.with_name(f".{target.name}.reserved")
            reserved.write_text("reserved")
            return reserved

        controller = FakeSystemctl(manifest, helper.UnitState(True, False, True, True, 41))
        with mock.patch.object(helper, "service_group_id", side_effect=group_id), \
             mock.patch.object(helper, "prepare_atomic_install", side_effect=reserve), \
             mock.patch.object(controller, "verify_units"):
            prepared = helper.prepare_installs(manifest, controller)
        self.assertIsNotNone(prepared.rollback_environment)
        self.assertIn((Path(manifest.targets.environment), previous_gid), owners)

    def test_first_install_rollback_disables_candidate_units(self):
        manifest = self.manifest()
        controller = FakeSystemctl(manifest, helper.UnitState(False, False, False, False, 0))
        with mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("candidate failed")):
            state = helper.activate(manifest, controller)
        self.assertEqual("activation_failed_rolled_back", state)
        self.assertIn(
            ("command", ("disable", controller.socket, controller.service), False),
            controller.events,
        )

    def test_first_install_rollback_removes_only_transaction_created_data_directory(self):
        manifest = dataclasses.replace(self.manifest(), data_directory_preexisting=False)
        self.policy.active_path.write_text(manifest.transaction_id + "\n")
        data_dir = Path(manifest.targets.deployed_sha).parent
        data_dir.mkdir(exist_ok=True)
        (data_dir / "transaction-created").write_text("remove me")
        controller = FakeSystemctl(manifest, helper.UnitState(False, False, False, False, 0))
        original_rmtree = shutil.rmtree
        claim_held_during_cleanup = []

        def remove(path, *args, **kwargs):
            if Path(path) == data_dir:
                claim_held_during_cleanup.append(self.policy.active_path.exists())
            return original_rmtree(path, *args, **kwargs)

        with mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("candidate failed")), \
             mock.patch.object(helper.shutil, "rmtree", side_effect=remove):
            state = helper.activate(manifest, controller)
        self.assertEqual("activation_failed_rolled_back", state)
        self.assertEqual([True], claim_held_during_cleanup)
        self.assertFalse(data_dir.exists())

    def test_rollback_failure_preserves_claim_and_both_failures(self):
        self.install_previous()
        manifest = self.manifest()
        self.policy.active_path.write_text(manifest.transaction_id + "\n")
        previous_state = helper.UnitState(True, False, True, True, 41)
        controller = FakeSystemctl(manifest, previous_state)
        controller.restore_state = mock.Mock(side_effect=helper.ActivationError("rollback start failed"))
        with mock.patch.object(
            helper,
            "wait_for_identity",
            side_effect=helper.ActivationError("candidate crashed"),
        ):
            state = helper.activate(manifest, controller)
        self.assertEqual("ordinary_activation_failed_rollback_failed", state)
        status = json.loads(self.policy.status_path.read_text())
        self.assertEqual("candidate crashed", status["failure"])
        self.assertEqual("rollback start failed", status["rollback_failure"])
        self.assertEqual(state, status["state"])
        self.assertEqual("a" * 40, Path(self.targets.deployed_sha).read_text().strip())
        self.assertEqual(manifest.transaction_id, self.policy.active_path.read_text().strip())
        self.assertTrue(helper.status_is_durable_terminal(manifest))
        self.assertFalse(helper.release_claim(manifest))

    def test_both_rollback_failure_statuses_retain_claim_and_reject_next_deployment(self):
        manifest = self.manifest()
        self.policy.active_path.write_text(manifest.transaction_id + "\n")
        for state in (
            "activation_failed_rollback_failed",
            "ordinary_activation_failed_rollback_failed",
        ):
            with self.subTest(state=state):
                helper.write_status(manifest, state, failure="candidate failed", rollback_failure="rollback failed")
                before = self.policy.status_path.read_bytes()
                self.assertTrue(helper.status_is_durable_terminal(manifest))
                self.assertFalse(helper.release_claim(manifest))
                with self.assertRaises(helper.ConcurrentDeploy):
                    helper.acquire_claim("f" * 32, self.policy)
                self.assertEqual(manifest.transaction_id, self.policy.active_path.read_text().strip())
                self.assertEqual(before, self.policy.status_path.read_bytes())

    def test_manifest_validation_failure_finalizes_status_and_releases_claim(self):
        manifest = self.manifest()
        self.policy.active_path.write_text(manifest.transaction_id + "\n")
        helper.write_policy_status(manifest, self.policy, "prepared")
        with mock.patch.object(sys, "argv", ["helper", "activate", "--manifest", str(self.manifest_path)]), \
             mock.patch.object(os, "geteuid", return_value=0), \
             mock.patch.object(helper.ValidationPolicy, "production", return_value=self.policy), \
             mock.patch.object(helper, "validate_manifest", side_effect=helper.ValidationError("invalid rollback URL")):
            self.assertEqual(1, helper.main())
        status = json.loads(self.policy.status_path.read_text())
        self.assertEqual("precondition_failed", status["state"])
        self.assertEqual("invalid rollback URL", status["failure"])
        self.assertFalse(self.policy.active_path.exists())

    def test_preparation_failure_does_not_stop_service(self):
        manifest = self.manifest()
        controller = FakeSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        controller.verify_units = mock.Mock(side_effect=helper.ActivationError("bad unit"))
        with self.assertRaisesRegex(helper.ActivationError, "bad unit"):
            helper.activate(manifest, controller)
        self.assertNotIn("stop", controller.events)
        self.assertEqual("precondition_failed", json.loads(self.policy.status_path.read_text())["state"])

    def test_partial_reservation_failure_cleans_temporary_files_before_disruption(self):
        manifest = self.manifest()
        controller = FakeSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        original = helper.prepare_atomic_install
        calls = 0

        def fail_after_first(*args, **kwargs):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise OSError("ENOSPC")
            return original(*args, **kwargs)

        with mock.patch.object(helper, "prepare_atomic_install", side_effect=fail_after_first):
            with self.assertRaisesRegex(OSError, "ENOSPC"):
                helper.activate(manifest, controller)
        self.assertNotIn("stop", controller.events)
        self.assertEqual([], list(Path(self.targets.binary).parent.glob(".*.install-*")))
        self.assertEqual("precondition_failed", json.loads(self.policy.status_path.read_text())["state"])

    def test_concurrent_activation_does_not_replace_active_status_or_claim(self):
        manifest = self.manifest()
        self.policy.active_path.write_text(manifest.transaction_id + "\n")
        helper.write_status(manifest, "activating")
        before = self.policy.status_path.read_bytes()
        with self.policy.activation_lock_path.open("a+") as lock:
            import fcntl

            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(helper.ConcurrentDeploy):
                helper.activate(manifest, FakeSystemctl(manifest, helper.UnitState(True, True, True, True, 41)))
        self.assertEqual(before, self.policy.status_path.read_bytes())
        self.assertEqual(manifest.transaction_id, self.policy.active_path.read_text().strip())

    def test_claim_release_requires_matching_terminal_status(self):
        manifest = self.manifest()
        self.policy.active_path.write_text("newer-transaction\n")
        helper.write_status(manifest, "committed")
        self.assertFalse(helper.release_claim(manifest))
        self.assertTrue(self.policy.active_path.exists())
        self.policy.active_path.write_text(manifest.transaction_id + "\n")
        self.assertTrue(helper.release_claim(manifest))
        self.assertFalse(self.policy.active_path.exists())


class ProtectedSystemctl(FakeSystemctl):
    def assert_inactive(self):
        if not self.disruption_started:
            raise helper.ActivationError("not inactive")

    def verify_protected_configuration(self):
        pass

    def inspect(self):
        self.events.append("inspect")
        if self.disruption_started:
            return helper.UnitState(False, False, False, False, 0)
        return self.previous_state


class SystemdProtectedCurrentTests(SystemdActivationTests):
    def protected(self):
        import platform
        linux = mock.patch.object(helper.sys, "platform", "linux")
        machine = mock.patch.object(platform, "machine", return_value="x86_64")
        linux.start()
        machine.start()
        self.addCleanup(linux.stop)
        self.addCleanup(machine.stop)
        self.install_previous()
        # macOS /var is a symlink; production evidence requires canonical paths.
        def canonical(value):
            if isinstance(value, dict):
                return {key: canonical(item) for key, item in value.items()}
            if isinstance(value, str) and value.startswith("/"):
                return str(Path(value).resolve())
            return value
        self.raw = canonical(self.raw)
        self.targets = helper.SystemdTargets(**self.raw["targets"])
        self.policy = dataclasses.replace(self.policy, targets=self.targets,
                                          transaction_root=self.policy.transaction_root.resolve(),
                                          status_path=self.policy.status_path.resolve(), active_path=self.policy.active_path.resolve(),
                                          activation_lock_path=self.policy.activation_lock_path.resolve(),
                                          claim_lock_path=self.policy.claim_lock_path.resolve())
        self.transaction = self.transaction.resolve()
        self.manifest_path = self.manifest_path.resolve()
        # Unit subprocess oracles use inert artifacts, never the production executable.
        asset_pin = mock.patch.object(helper, "PROTECTED_ASSET_SHA256", self.raw["candidate"]["binary"]["sha256"])
        registry_pin = mock.patch.object(helper, "PROTECTED_REGISTRY_SHA256", digest(ROOT / "crates/phoenix-db/src/migrations.rs"))
        asset_pin.start()
        registry_pin.start()
        self.addCleanup(asset_pin.stop)
        self.addCleanup(registry_pin.stop)
        self.raw.update(manifest_version=helper.PROTECTED_HANDOFF_PROTOCOL_VERSION,
                        source_commit=helper.PROTECTED_RELEASE_COMMIT,
                        expected={"version": helper.PROTECTED_RELEASE_VERSION, "git_sha": helper.PROTECTED_RELEASE_COMMIT},
                        mode=helper.PROTECTED_MODE, source_kind="published_release",
                        release_tag=helper.PROTECTED_RELEASE_TAG, release_commit=helper.PROTECTED_RELEASE_COMMIT,
                        controller_source_commit="c" * 40,
                        controller_helper=self.artifact("helper.py", (ROOT / "scripts/systemd_deploy_helper.py").read_text()),
                        registry=self.artifact("migrations.rs", (ROOT / "crates/phoenix-db/src/migrations.rs").read_text()),
                        database_path=str(Path(self.targets.deployed_sha).parent / "prod.db"),
                        release_asset_name="phoenix_ide-x86_64-unknown-linux-musl",
                        release_asset_sha256=self.raw["candidate"]["binary"]["sha256"])
        service = (f"[Service]\nUser={self.user}\nExecStart={self.targets.binary}\n"
                   f"Environment=PHOENIX_DB_PATH={self.raw['database_path']}\n"
                   f"EnvironmentFile={self.targets.environment}\n")
        environment = f"SECRET=installed\nPHOENIX_DB_PATH={self.raw['database_path']}\n"
        for name, content in (("service", service), ("socket", "installed socket"), ("environment", environment)):
            Path(getattr(self.targets, name)).write_text(content)
            self.raw["candidate"][name] = self.artifact("candidate-" + name, content)
            self.raw["rollback"][name] = self.artifact("rollback-" + name, content)
        self.write_manifest()
        manifest = self.manifest()
        self.registry = helper.candidate_registry(manifest)
        self.database = Path(manifest.database_path)
        import sqlite3
        with sqlite3.connect(self.database) as db:
            db.execute("CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            db.executemany("INSERT INTO _migrations VALUES (?, ?)", self.registry[:65])
            db.execute("CREATE TABLE data(value BLOB)")
            db.execute("INSERT INTO data VALUES (x'cafe')")
        Path(manifest.active_path).write_text(manifest.transaction_id + "\n")
        helper.write_status(manifest, "prepared")
        return manifest

    def advance(self, end=None, *, divergent=False, database=None):
        import sqlite3
        with sqlite3.connect(database or self.database) as db:
            current = db.execute("SELECT max(version) FROM _migrations").fetchone()[0]
            rows = [row for row in self.registry if row[0] > current and (end is None or row[0] <= end)]
            db.executemany("INSERT INTO _migrations VALUES (?, ?)", rows)
            if divergent:
                db.execute("UPDATE _migrations SET name='divergent' WHERE version=66")

    def runtime(self, manifest, failure=None):
        def run(command, **kwargs):
            if command[0] == "lsof":
                return subprocess.CompletedProcess(command, 1, "", "")
            if command[0] == manifest.candidate.binary.path:
                self.assertEqual([manifest.candidate.binary.path, "--migrate-only"], command)
                self.assertFalse((self.transaction / "migration_started").exists())
                self.assertEqual("old binary", Path(manifest.targets.binary).read_text())
                self.assertEqual({"PATH", "HOME", "PHOENIX_DB_PATH"}, set(kwargs["env"]))
                self.assertEqual(self.transaction / "rehearsal.sqlite", Path(kwargs["env"]["PHOENIX_DB_PATH"]))
                self.advance(database=kwargs["env"]["PHOENIX_DB_PATH"])
                return subprocess.CompletedProcess(command, 0, "", "")
            self.assertEqual([manifest.targets.binary, "--migrate-only"], command)
            self.assertEqual(helper.transaction_context(manifest), json.loads((self.transaction / "migration_started").read_text()))
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("activating", status["state"])
            self.assertTrue(status["migration_started"])
            account = pwd.getpwnam(manifest.service_user)
            self.assertEqual(account.pw_uid, kwargs["user"])
            self.assertEqual(account.pw_gid, kwargs["group"])
            self.assertEqual(manifest.database_path, kwargs["env"]["PHOENIX_DB_PATH"])
            self.assertEqual("installed", kwargs["env"]["SECRET"])
            self.assertNotIn("LISTEN_FDS", kwargs["env"])
            if failure == "spawn":
                raise OSError("spawn refused")
            if failure == "nonzero":
                raise subprocess.CalledProcessError(1, command)
            if failure == "partial":
                self.advance(80)
                raise subprocess.CalledProcessError(1, command)
            self.advance(divergent=failure == "target")
            return subprocess.CompletedProcess(command, 0, "", "")
        return run

    def execute(self, manifest, controller, failure=None):
        with mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest, failure)), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
             mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("health") if failure == "health" else None):
            return helper.activate(manifest, controller)

    def reboot_start_allowed(self, path, *, manual=False):
        import configparser
        if not path.exists():
            return True
        config = configparser.ConfigParser(interpolation=None)
        config.read(path)
        if manual and config["Unit"].get("RefuseManualStart") == "yes":
            return False
        condition = config["Unit"].get("ConditionPathExists")
        return not Path(condition[1:]).exists() if condition.startswith("!") else Path(condition).exists()

    def assert_reboot_inhibited(self, manifest):
        helper.verify_start_inhibit_files(manifest)
        for path in helper.start_inhibit_paths(manifest):
            content = path.read_text()
            self.assertIn("\nRefuseManualStart=yes\n", content)
            self.assertIn("\nConditionPathExists=!/\n", content)
            self.assertIn(manifest.transaction_id, content)
            self.assertIn(helper.transaction_context(manifest)["manifest_sha256"], content)
            self.assertFalse(self.reboot_start_allowed(path), "reboot dependency activation must be inhibited")
            self.assertFalse(self.reboot_start_allowed(path, manual=True), "manual systemctl start must be inhibited")
            self.assertEqual(0o644, path.stat().st_mode & 0o777)

    def test_power_loss_at_each_disrupted_boundary_retains_persistent_reboot_and_manual_start_inhibit(self):
        class PowerLoss(BaseException):
            pass
        for window in ("stop", "snapshot_current", "rehearse_current_migration", "install_candidate", "marker", "partial"):
            with self.subTest(window=window):
                manifest = self.protected()
                ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
                runtime = self.runtime(manifest)
                def run(command, **kwargs):
                    if window == "partial" and command == [manifest.targets.binary, "--migrate-only"]:
                        self.advance(80)
                        raise PowerLoss()
                    return runtime(command, **kwargs)
                original_write = helper.atomic_write
                def write(path, *args, **kwargs):
                    original_write(path, *args, **kwargs)
                    if window == "marker" and path.name == "migration_started":
                        raise PowerLoss()
                target = ctl if window == "stop" else helper
                name = window if window in {"stop", "snapshot_current", "rehearse_current_migration", "install_candidate"} else "utc_now"
                original = getattr(target, name)
                def interrupted(*args, **kwargs):
                    value = original(*args, **kwargs)
                    if window not in {"marker", "partial"}:
                        self.assert_reboot_inhibited(manifest)
                        raise PowerLoss()
                    return value
                with mock.patch.object(target, name, side_effect=interrupted), \
                     mock.patch.object(helper, "atomic_write", side_effect=write), \
                     mock.patch.object(helper.subprocess, "run", side_effect=run), \
                     mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
                     mock.patch.object(helper, "wait_for_identity"), \
                     self.assertRaises(PowerLoss):
                    helper.activate(manifest, ctl)
                self.assert_reboot_inhibited(manifest)
                self.assertTrue(Path(manifest.active_path).exists())
                self.assertFalse(any(isinstance(event, tuple) and event[0] in {"restore_state", "start_candidate"} for event in ctl.events))
                self.database.unlink()
                for name in ("source.sqlite", "source-proof.json", "rehearsal.sqlite", "rehearsal-proof.json", "migration_started"):
                    (self.transaction / name).unlink(missing_ok=True)
                for path in helper.start_inhibit_paths(manifest):
                    path.unlink()

    def test_both_inhibits_are_fsynced_reloaded_and_verified_before_first_stop(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        events = []
        write, sync, stop = helper.atomic_write, helper.fsync_dir, ctl.stop
        def durable_write(path, *args, **kwargs):
            write(path, *args, **kwargs)
            events.append(("written", path))
        def durable_sync(path):
            sync(path)
            events.append(("synced", path))
        def stopped():
            self.assert_reboot_inhibited(manifest)
            self.assertIn("reload", ctl.events)
            for path in helper.start_inhibit_paths(manifest):
                self.assertIn(("written", path), events)
                self.assertIn(("synced", path.parent), events)
                self.assertIn(("synced", path.parent.parent), events)
            stop()
        with mock.patch.object(helper, "atomic_write", side_effect=durable_write), \
             mock.patch.object(helper, "fsync_dir", side_effect=durable_sync), \
             mock.patch.object(ctl, "stop", side_effect=stopped):
            self.assertEqual("committed", self.execute(manifest, ctl))
        self.assertTrue(all(not path.exists() for path in helper.start_inhibit_paths(manifest)))
        for path in helper.start_inhibit_paths(manifest):
            self.assertTrue(self.reboot_start_allowed(path))
            self.assertTrue(self.reboot_start_allowed(path, manual=True))

    def test_incomplete_or_unverified_inhibit_setup_cannot_stop_predecessor_or_release_claim(self):
        manifest = self.protected()
        paths = helper.start_inhibit_paths(manifest)
        for failure in ("second_write", "reload", "verify"):
            ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
            helper.write_status(manifest, "prepared")
            write = helper.atomic_write
            def failed_write(path, *args, **kwargs):
                if failure == "second_write" and path == paths[1]:
                    raise OSError("cannot persist socket inhibit")
                write(path, *args, **kwargs)
            original_verify = ctl.verify_protected_configuration
            def verify():
                if failure == "verify" and paths[0].exists():
                    raise helper.ActivationError("loaded inhibit not verified")
                original_verify()
            with self.subTest(failure=failure), \
                 mock.patch.object(helper, "atomic_write", side_effect=failed_write), \
                 mock.patch.object(ctl, "daemon_reload", side_effect=OSError("reload failed") if failure == "reload" else None), \
                 mock.patch.object(ctl, "verify_protected_configuration", side_effect=verify):
                self.assertEqual("activation_failed_rollback_failed", helper.activate(manifest, ctl))
            self.assertNotIn("stop", ctl.events)
            self.assertFalse(helper.release_claim(manifest))
            self.assertEqual("old binary", Path(manifest.targets.binary).read_text())
            for path in paths:
                path.unlink(missing_ok=True)

    def test_post_marker_recovery_stops_first_for_every_detailed_validation_failure(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        self.assertEqual("current_forward_failed", self.execute(manifest, ctl, "partial"))
        paths = [Path(manifest.status_path), self.transaction / "migration_started",
                 self.transaction / "source-proof.json", self.transaction / "rehearsal-proof.json",
                 Path(manifest.targets.environment), Path(manifest.targets.binary),
                 *helper.start_inhibit_paths(manifest)]
        for path in paths:
            original = path.read_bytes()
            path.write_bytes(b"tamper")
            ctl.events.clear()
            with self.subTest(path=path.name), \
                 mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest)), \
                 mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
                 mock.patch.object(helper, "restore", side_effect=AssertionError("forbidden")):
                self.assertEqual("current_forward_failed", helper.recover_current(manifest, ctl))
            self.assertEqual("stop", ctl.events[0])
            self.assertEqual("stop", ctl.events[-1])
            self.assertEqual("current_forward_failed", json.loads(Path(manifest.status_path).read_text())["state"])
            self.assertTrue(Path(manifest.active_path).exists())
            self.assertFalse(any(isinstance(event, tuple) and event[0] in {"restore_state", "start_candidate"} for event in ctl.events))
            path.write_bytes(original)
        helper.verify_start_inhibit_files(manifest)
        for path in helper.start_inhibit_paths(manifest):
            original = path.read_bytes()
            path.unlink()
            ctl.events.clear()
            self.assertEqual("current_forward_failed", helper.recover_current(manifest, ctl))
            self.assertEqual("stop", ctl.events[0])
            self.assertEqual("stop", ctl.events[-1])
            self.assertEqual(original, path.read_bytes())

    def test_recovery_cli_defers_candidate_artifact_validation_until_after_stop(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        self.assertEqual("current_forward_failed", self.execute(manifest, ctl, "partial"))
        Path(manifest.candidate.binary.path).write_bytes(b"changed retained candidate")
        ctl.events.clear()
        with mock.patch.object(helper.os, "geteuid", return_value=0), \
             mock.patch.object(helper.ValidationPolicy, "production", return_value=self.policy), \
             mock.patch.object(helper, "Systemctl", return_value=ctl), \
             mock.patch.object(helper, "SystemdUnitControl", return_value=ctl), \
             mock.patch.object(helper.sys, "argv", ["helper.py", "recover-current", "--manifest", str(self.manifest_path)]):
            self.assertEqual(1, helper.main())
        self.assertEqual("stop", ctl.events[0])
        self.assertEqual("stop", ctl.events[-1])
        self.assertIn("candidate binary checksum", json.loads(Path(manifest.status_path).read_text())["failure"])
        self.assertTrue(Path(manifest.active_path).exists())

    def test_recovery_cli_stops_running_candidate_before_malformed_manifest_decode(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        self.assertEqual("current_forward_failed", self.execute(manifest, ctl, "partial"))
        original_manifest = self.manifest_path.read_bytes()
        marker = self.transaction / "migration_started"
        original_marker = marker.read_bytes()
        load = helper.Manifest.load
        for marker_kind, contents in (("file", b'{"expected":'), ("file", b'{"expected":null}'),
                                      ("symlink", b'{'), ("directory", b'{')):
            marker.unlink()
            if marker_kind == "symlink":
                marker.symlink_to(self.transaction / "missing-marker-target")
            elif marker_kind == "directory":
                marker.mkdir()
            else:
                marker.write_bytes(b"malformed marker")
            self.manifest_path.write_bytes(contents)
            running = {"service": True, "socket": True, "pid": 52}
            commands = []
            def run(command, **_kwargs):
                commands.append(command)
                self.assertEqual("systemctl", command[0])
                if command[1] == "stop":
                    self.assertEqual([f"{self.policy.unit_name}.service", f"{self.policy.unit_name}.socket"], command[2:])
                    running.update(service=False, socket=False, pid=0)
                    return subprocess.CompletedProcess(command, 0, "", "")
                if command[1] == "is-active":
                    value = "active" if running["service" if command[2].endswith(".service") else "socket"] else "inactive"
                elif command[3] == "--property=MainPID":
                    value = str(running["pid"])
                else:
                    self.assertEqual("--property=ActiveState", command[3])
                    value = "active" if running["service" if command[2].endswith(".service") else "socket"] else "inactive"
                return subprocess.CompletedProcess(command, 0, value + "\n", "")
            fixed = helper.SystemdUnitControl(self.policy.unit_name, run=run)
            def decode(path):
                self.assertEqual({"service": False, "socket": False, "pid": 0}, running)
                self.assertTrue(any("--property=ActiveState" in command for command in commands))
                return load(path)
            with self.subTest(marker=marker_kind, contents=contents), \
                 mock.patch.object(helper.os, "geteuid", return_value=0), \
                 mock.patch.object(helper.ValidationPolicy, "production", return_value=self.policy), \
                 mock.patch.object(helper, "SystemdUnitControl", return_value=fixed), \
                 mock.patch.object(helper.Manifest, "load", side_effect=decode), \
                 mock.patch.object(helper, "Systemctl", side_effect=AssertionError("manifest was not decoded")), \
                 mock.patch.object(helper.sys, "argv", ["helper.py", "recover-current", "--manifest", str(self.manifest_path)]):
                self.assertEqual(1, helper.main())
            self.assertFalse(any(running.values()))
            self.assertEqual(manifest.transaction_id, Path(manifest.active_path).read_text().strip())
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("current_forward_failed", status["state"])
            self.assertTrue(status["migration_started"])
            self.assertIn("manifest shape", status["failure"])
            self.manifest_path.write_bytes(original_manifest)
            if marker_kind == "directory":
                marker.rmdir()
            else:
                marker.unlink()
            marker.write_bytes(original_marker)

    def test_recovery_cli_requires_exact_claim_and_marker_before_stop_or_decode(self):
        manifest = self.protected()
        marker = self.transaction / "migration_started"
        for scenario in ("prepared", "foreign-claim", "symlink-claim", "foreign-path"):
            requested = self.manifest_path
            Path(manifest.active_path).unlink(missing_ok=True)
            Path(manifest.active_path).write_text(manifest.transaction_id + "\n")
            marker.unlink(missing_ok=True)
            if scenario != "prepared":
                marker.write_text("invalid")
            if scenario == "foreign-claim":
                Path(manifest.active_path).write_text("f" * 32)
            elif scenario == "symlink-claim":
                Path(manifest.active_path).unlink()
                Path(manifest.active_path).symlink_to(self.transaction / "unrelated")
            elif scenario == "foreign-path":
                requested = self.transaction / "other.json"
            with self.subTest(scenario=scenario), \
                 mock.patch.object(helper.Manifest, "load", side_effect=AssertionError("must not decode")), \
                 mock.patch.object(helper, "SystemdUnitControl") as control:
                with self.assertRaises(helper.ValidationError):
                    helper.recover_current_path(requested, self.policy)
            control.assert_not_called()

    def test_loaded_systemd_inhibit_requires_exact_dropins_reload_and_manual_refusal(self):
        manifest = self.protected()
        fake = ProtectedSystemctl(manifest, helper.UnitState(False, True, False, True, 0))
        helper.install_start_inhibit(manifest, fake)
        ctl = helper.Systemctl(manifest)
        paths = helper.start_inhibit_paths(manifest)
        values = {(unit, key): value for unit, fragment, path in
                  ((ctl.service, manifest.targets.service, paths[0]), (ctl.socket, manifest.targets.socket, paths[1]))
                  for key, value in (("FragmentPath", fragment), ("DropInPaths", str(path)),
                                     ("NeedDaemonReload", "no"), ("RefuseManualStart", "yes"))}
        values.update({(ctl.service, "User"): manifest.service_user, (ctl.service, "MainPID"): "0"})
        with mock.patch.object(ctl, "property", side_effect=lambda unit, key: values[(unit, key)]):
            ctl.verify_protected_configuration()
        for unit in (ctl.service, ctl.socket):
            for key, bad in (("DropInPaths", ""), ("DropInPaths", str(paths[0]) + " /foreign.conf"),
                             ("NeedDaemonReload", "yes"), ("RefuseManualStart", "no")):
                mutated = {**values, (unit, key): bad}
                with self.subTest(unit=unit, key=key), \
                     mock.patch.object(ctl, "property", side_effect=lambda unit, key: mutated[(unit, key)]), \
                     self.assertRaises(helper.ActivationError):
                    ctl.verify_protected_configuration()

    def test_no_marker_recovery_does_not_stop_predecessor_even_with_bad_status(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        for status in (Path(manifest.status_path).read_bytes(), b"invalid"):
            Path(manifest.status_path).write_bytes(status)
            with self.assertRaisesRegex(helper.ValidationError, "requires irreversible marker"):
                helper.recover_current(manifest, ctl)
            self.assertEqual([], ctl.events)

    def test_start_inhibit_binds_exact_paths_and_rejects_content_mode_symlink_or_other_transaction(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        helper.install_start_inhibit(manifest, ctl)
        for path in helper.start_inhibit_paths(manifest):
            original = path.read_bytes()
            for contents in (original.replace(b"ConditionPathExists=!/", b"ConditionPathExists=/"),
                             original.replace(manifest.transaction_id.encode(), b"f" * 32),
                             original.replace(str(path).encode(), b"/wrong/path")):
                path.write_bytes(contents)
                with self.assertRaisesRegex(helper.ValidationError, "start inhibit"):
                    helper.verify_start_inhibit_files(manifest)
                with self.assertRaises(helper.ValidationError):
                    helper.install_start_inhibit(manifest, ctl)
                self.assertEqual(contents, path.read_bytes(), "must not overwrite foreign/tampered inhibit")
            path.write_bytes(original)
            path.chmod(0o666)
            with self.assertRaises(helper.ValidationError):
                helper.verify_start_inhibit_files(manifest)
            path.chmod(0o644)
            path.unlink()
            path.symlink_to(self.transaction / "unrelated")
            with self.assertRaises(helper.ValidationError):
                helper.verify_start_inhibit_files(manifest)
            path.unlink()
            path.write_bytes(original)
            path.chmod(0o644)

    def test_protected_snapshot_stop_marker_and_preserved_configuration(self):
        manifest = self.protected()
        controller = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        before = {name: Path(getattr(self.targets, name)).read_bytes() for name in ("service", "socket", "environment")}
        snapshot = helper.snapshot_current
        def take(*args):
            self.assertIn("stop", controller.events)
            self.assertEqual("old binary", Path(self.targets.binary).read_text())
            self.assertFalse((self.transaction / "migration_started").exists())
            evidence = snapshot(*args)
            self.assertEqual(helper.database_evidence(self.database), evidence)
            return evidence
        with mock.patch.object(helper, "snapshot_current", side_effect=take):
            self.assertEqual("committed", self.execute(manifest, controller))
        proof = helper.verify_snapshot_proof(manifest)
        self.assertEqual(65, len(proof["source_ledger"]))
        helper.verify_rehearsal_proof(manifest)
        rehearsal = json.loads((self.transaction / "rehearsal-proof.json").read_text())
        self.assertTrue(rehearsal["success"])
        self.assertEqual(proof["backup_sha256"], rehearsal["backup_sha256"])
        self.assertEqual(0o400, (self.transaction / "source.sqlite").stat().st_mode & 0o777)
        self.assertEqual(self.registry, helper.database_evidence(self.database)[0])
        for name, contents in before.items():
            self.assertEqual(contents, Path(getattr(self.targets, name)).read_bytes())
        self.assertTrue(helper.release_claim(manifest))
        self.assertTrue((self.transaction / "source.sqlite").exists())
        self.assertTrue((self.transaction / "source-proof.json").exists())

    def test_forged_source_ledger_is_rejected_by_structural_rehearsal_before_marker(self):
        import sqlite3
        manifest = self.protected()
        before = helper.database_evidence(self.database)
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        def run(command, **kwargs):
            if command[0] == "lsof":
                return subprocess.CompletedProcess(command, 1, "", "")
            self.assertEqual([manifest.candidate.binary.path, "--migrate-only"], command)
            self.assertIn("stop", ctl.events)
            self.assertFalse((self.transaction / "migration_started").exists())
            self.assertEqual("old binary", Path(manifest.targets.binary).read_text())
            self.assertTrue((self.transaction / "source-proof.json").is_file())
            rehearsal = kwargs["env"]["PHOENIX_DB_PATH"]
            self.assertNotEqual(str(self.database), rehearsal)
            self.assertEqual(before, helper.database_evidence(Path(rehearsal)))
            # Simulate the retained binary's structural migration failure on the forged schema.
            with sqlite3.connect(rehearsal) as db:
                with self.assertRaises(sqlite3.OperationalError):
                    db.execute("ALTER TABLE conversations ADD COLUMN migrated TEXT")
            raise subprocess.CalledProcessError(1, command, stderr="no such table: conversations")
        with mock.patch.object(helper.subprocess, "run", side_effect=run), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
             mock.patch.object(helper, "wait_for_identity"):
            self.assertEqual("activation_failed_rolled_back", helper.activate(manifest, ctl))
        self.assertFalse((self.transaction / "migration_started").exists())
        self.assertEqual(before, helper.database_evidence(self.database))
        self.assertEqual(before, helper.database_evidence(self.transaction / "source.sqlite"))
        self.assertFalse(json.loads((self.transaction / "rehearsal-proof.json").read_text())["success"])
        self.assertEqual("old binary", Path(manifest.targets.binary).read_text())

    def test_rehearsal_must_reach_exact_target_with_clean_foreign_keys(self):
        import sqlite3
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        ctl.stop()
        with mock.patch.object(helper, "assert_database_exclusive"):
            helper.snapshot_current(manifest, ctl)
        source_hash = digest(self.transaction / "source.sqlite")
        for failure in ("incomplete", "foreign_key"):
            def run(command, **kwargs):
                path = kwargs["env"]["PHOENIX_DB_PATH"]
                if failure == "foreign_key":
                    self.advance(database=path)
                    with sqlite3.connect(path) as db:
                        db.executescript("CREATE TABLE parent(id PRIMARY KEY); CREATE TABLE child(id REFERENCES parent(id)); INSERT INTO child VALUES (1);")
                return subprocess.CompletedProcess(command, 0, "", "")
            with self.subTest(failure=failure), \
                 mock.patch.object(helper.subprocess, "run", side_effect=run), \
                 mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
                 self.assertRaises(helper.ActivationError):
                helper.rehearse_current_migration(manifest)
            self.assertEqual(source_hash, digest(self.transaction / "source.sqlite"))
            self.assertFalse((self.transaction / "migration_started").exists())
            self.assertFalse(json.loads((self.transaction / "rehearsal-proof.json").read_text())["success"])

    def test_rehearsal_proof_or_retained_database_tamper_blocks_live_marker(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        ctl.stop()
        with mock.patch.object(helper, "assert_database_exclusive"), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
             mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest)):
            helper.snapshot_current(manifest, ctl)
            helper.rehearse_current_migration(manifest)
        proof_path = self.transaction / "rehearsal-proof.json"
        proof = json.loads(proof_path.read_text())
        for mutation in ({"transaction_id": "f" * 32}, {"success": False}, {"backup_sha256": "a" * 64}):
            proof_path.write_text(json.dumps({**proof, **mutation}))
            with self.subTest(mutation=mutation), mock.patch.object(helper.subprocess, "run") as run:
                with self.assertRaisesRegex(helper.ActivationError, "rehearsal proof"):
                    helper.run_current_migration(manifest, ctl)
                run.assert_not_called()
            self.assertFalse((self.transaction / "migration_started").exists())
        proof_path.write_text(json.dumps(proof))
        rehearsal = self.transaction / "rehearsal.sqlite"
        rehearsal.chmod(0o600)
        with rehearsal.open("ab") as stream:
            stream.write(b"tampered")
        with self.assertRaisesRegex(helper.ActivationError, "rehearsal proof"):
            helper.run_current_migration(manifest, ctl)
        self.assertFalse((self.transaction / "migration_started").exists())

    def test_protected_registry_rejects_earlier_name_and_sql_byte_changes_by_hash(self):
        manifest = self.protected()
        path = Path(manifest.registry.path)
        source = path.read_text()
        for variant in (source.replace('name: "', 'name: "changed_', 1), source.replace("CREATE TABLE", "CREATE  TABLE", 1)):
            self.assertNotEqual(source, variant)
            path.write_text(variant)
            mutated = dataclasses.replace(manifest, registry=helper.Artifact(str(path), digest(path)))
            with self.assertRaisesRegex(helper.ActivationError, "registry hash is not the reviewed release"):
                helper.candidate_registry(mutated)
        path.write_text(source)

    def test_protected_pins_and_protocol_are_independently_enforced_before_stop(self):
        manifest = self.protected()
        mutations = [dict(release_tag="v0.13.0-rc.3"), dict(source_commit="d" * 40, release_commit="d" * 40),
                     dict(expected=helper.Identity("0.13.0-rc.5", manifest.source_commit)),
                     dict(release_asset_name="other"), dict(release_asset_sha256="a" * 64),
                     dict(candidate=dataclasses.replace(manifest.candidate, binary=helper.Artifact(manifest.candidate.binary.path, "a" * 64))),
                     dict(manifest_version=1), dict(registry=helper.Artifact(manifest.registry.path, "a" * 64))]
        for mutation in mutations:
            mutated = dataclasses.replace(manifest, **mutation)
            ctl = ProtectedSystemctl(mutated, helper.UnitState(True, True, True, True, 41))
            with self.subTest(mutation=mutation), self.assertRaises(helper.ValidationError):
                helper.activate(mutated, ctl)
            self.assertNotIn("stop", ctl.events)

    def test_ordinary_future_registry129_exact_target_and_no_pending(self):
        import sqlite3
        manifest = self.protected()
        registry_path = Path(manifest.registry.path)
        source = registry_path.read_text()
        source = source.replace('\n];', '\n    Migration { version: 129, name: "future", sql: "SELECT 1", },\n];', 1)
        registry_path.write_text(source)
        ordinary = dataclasses.replace(manifest, mode="ordinary", manifest_version=1, controller_source_commit=None,
                                       controller_helper=None, release_asset_name=None, release_asset_sha256=None,
                                       registry=helper.Artifact(str(registry_path), digest(registry_path)))
        registry = helper.candidate_registry(ordinary)
        self.assertEqual((129, "future"), registry[-1])
        self.advance()
        with self.assertRaisesRegex(helper.ActivationError, "pending migration interval"):
            helper.database_preflight(ordinary)
        with sqlite3.connect(self.database) as db:
            db.execute("INSERT INTO _migrations VALUES (129, 'future')")
        ctl = ProtectedSystemctl(ordinary, helper.UnitState(True, True, True, True, 41))
        with mock.patch.object(helper, "wait_for_identity"):
            self.assertEqual("committed", helper.activate(ordinary, ctl))
        self.assertFalse((self.transaction / "migration_started").exists())

    def test_ordinary_registry_rejects_unsorted_duplicate_and_empty_sets(self):
        manifest = self.protected()
        path = Path(manifest.registry.path)
        ordinary = dataclasses.replace(manifest, mode="ordinary")
        for versions in ((2, 1), (1, 1), (), (0,)):
            entries = "\n".join(f'Migration {{ version: {v}, name: "n{v}", sql: X, }},' for v in versions)
            path.write_text("const MIGRATIONS: &[Migration] = &[\n" + entries + "\n];")
            value = dataclasses.replace(ordinary, registry=helper.Artifact(str(path), digest(path)))
            with self.subTest(versions=versions), self.assertRaisesRegex(helper.ActivationError, "sorted and unique"):
                helper.candidate_registry(value)

    def test_ordinary_v1_no_registry_gets_candidate_target_offline_without_protocol_fields(self):
        import sqlite3
        manifest = self.protected()
        ordinary = dataclasses.replace(manifest, mode="ordinary", manifest_version=1, controller_source_commit=None,
                                       controller_helper=None, release_asset_name=None, release_asset_sha256=None, registry=None)
        def run(command, **kwargs):
            self.assertEqual([manifest.candidate.binary.path, "--migrate-only"], command)
            self.assertEqual({"PATH", "HOME", "PHOENIX_DB_PATH"}, set(kwargs["env"]))
            path = Path(kwargs["env"]["PHOENIX_DB_PATH"])
            self.assertNotEqual(self.database, path)
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT)")
                db.executemany("INSERT INTO _migrations VALUES (?, ?)", self.registry)
            return subprocess.CompletedProcess(command, 0, "", "")
        with mock.patch.object(helper.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(helper.ActivationError, "pending migration interval"):
                helper.database_preflight(ordinary)
            self.advance()
            self.assertEqual(self.registry, helper.database_preflight(ordinary)[0])

    def test_every_post_marker_failure_retains_candidate_and_never_predecessor(self):
        for failure in ("spawn", "nonzero", "partial", "target", "start", "health"):
            with self.subTest(failure=failure):
                manifest = self.protected()
                ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41),
                                        start_failure="start failed" if failure == "start" else None)
                with mock.patch.object(helper, "restore", side_effect=AssertionError("predecessor forbidden")):
                    self.assertEqual("current_forward_failed", self.execute(manifest, ctl, failure))
                self.assertEqual(manifest.candidate.binary.sha256, helper.sha256(Path(self.targets.binary)))
                self.assertTrue((self.transaction / "source.sqlite").exists())
                self.assertTrue((self.transaction / "migration_started").exists())
                self.assertFalse(helper.release_claim(manifest))
                self.assertEqual("stop", ctl.events[-1])
                self.assert_reboot_inhibited(manifest)
                # Reset only this isolated fixture between fault injections.
                self.database.unlink()
                (self.transaction / "source.sqlite").unlink()
                (self.transaction / "source-proof.json").unlink()
                (self.transaction / "rehearsal.sqlite").unlink()
                (self.transaction / "rehearsal-proof.json").unlink()
                (self.transaction / "migration_started").unlink()
                for path in helper.start_inhibit_paths(manifest):
                    path.unlink()

    def test_partial_prefix_recovery_runs_only_candidate_and_target_recovery_skips_migration(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        self.assertEqual("current_forward_failed", self.execute(manifest, ctl, "partial"))
        backup_hash = helper.sha256(self.transaction / "source.sqlite")
        migrate = helper.run_current_migration
        def migrate_recovery(*args):
            status = helper.retained_status(manifest)
            self.assertEqual("activating", status["state"])
            self.assertTrue(status["migration_started"])
            return migrate(*args)
        with mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest)) as run, \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
             mock.patch.object(helper, "run_current_migration", side_effect=migrate_recovery), \
             mock.patch.object(helper, "wait_for_identity"), \
             mock.patch.object(helper, "restore", side_effect=AssertionError("forbidden")):
            self.assertEqual("committed", helper.recover_current(manifest, ctl))
        self.assertEqual(1, len([call for call in run.call_args_list if "--migrate-only" in call.args[0]]))
        helper.write_status(manifest, "current_forward_failed", failure="interrupted publication")
        helper.install_start_inhibit(manifest, ctl)
        authorize = helper.authorize_candidate_start
        def authorize_recovery(*args):
            status = helper.retained_status(manifest)
            self.assertEqual("activating", status["state"])
            self.assertTrue(status["migration_started"])
            self.assert_reboot_inhibited(manifest)
            return authorize(*args)
        with mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest)), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
             mock.patch.object(helper, "run_current_migration", side_effect=AssertionError("target is already exact")), \
             mock.patch.object(helper, "authorize_candidate_start", side_effect=authorize_recovery) as start, \
             mock.patch.object(helper, "write_status", wraps=helper.write_status) as status_write, \
             mock.patch.object(helper, "wait_for_identity"):
            self.assertEqual("committed", helper.recover_current(manifest, ctl))
        start.assert_called_once_with(manifest, ctl)
        self.assertEqual([mock.call(manifest, "activating"), mock.call(manifest, "committed")],
                         status_write.call_args_list)
        self.assertEqual(backup_hash, helper.sha256(self.transaction / "source.sqlite"))
        self.assertTrue(helper.release_claim(manifest))

    def test_source_ledger_and_ordinary_pending_refuse_before_stop(self):
        import sqlite3
        manifest = self.protected()
        for version in (0, 64, 66, 123, 128):
            with self.subTest(version=version), sqlite3.connect(self.database) as db:
                db.execute("DELETE FROM _migrations")
                db.executemany("INSERT INTO _migrations VALUES (?, ?)", [row for row in self.registry if row[0] <= version])
            ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
            helper.write_status(manifest, "prepared")
            with self.assertRaisesRegex(helper.ActivationError, "prefix65"):
                helper.activate(manifest, ctl)
            self.assertNotIn("stop", ctl.events)
        with sqlite3.connect(self.database) as db:
            db.execute("DELETE FROM _migrations")
            db.executemany("INSERT INTO _migrations VALUES (?, ?)", self.registry[:65])
        ordinary = dataclasses.replace(manifest, manifest_version=1, mode="ordinary", controller_source_commit=None,
                                       controller_helper=None, release_asset_name=None, release_asset_sha256=None)
        ctl = ProtectedSystemctl(ordinary, helper.UnitState(True, True, True, True, 41))
        with self.assertRaisesRegex(helper.ActivationError, "not qualified"):
            helper.activate(ordinary, ctl)
        self.assertNotIn("stop", ctl.events)
        self.advance()
        with mock.patch.object(helper, "wait_for_identity"):
            self.assertEqual("committed", helper.activate(ordinary, ctl))

    def test_first_install_with_existing_database_is_rejected_before_stop(self):
        manifest = self.protected()
        first = dataclasses.replace(manifest, manifest_version=1, mode="ordinary", previous=None, controller_source_commit=None,
                                    controller_helper=None, release_asset_name=None, release_asset_sha256=None)
        ctl = ProtectedSystemctl(first, helper.UnitState(False, False, False, False, 0))
        with self.assertRaisesRegex(helper.ActivationError, "without predecessor"):
            helper.activate(first, ctl)
        self.assertNotIn("stop", ctl.events)

    def test_registry_missing_extra_renamed_or_malformed_is_rejected(self):
        manifest = self.protected()
        source = Path(manifest.registry.path).read_text()
        variants = [source.replace('name: "product_creation_provisioning_stage"', 'name: "wrong"'),
                    source.replace("version: 128,", "version: 129,"),
                    source.replace("version: 65,", "version: 64,"),
                    source.replace("sql: MIGRATION_065,", "sql: unknown!(65),")]
        for variant in variants:
            with self.subTest(variant=variant[-20:]):
                Path(manifest.registry.path).write_text(variant)
                mutated = dataclasses.replace(manifest, registry=helper.Artifact(manifest.registry.path, helper.sha256(Path(manifest.registry.path))))
                ctl = ProtectedSystemctl(mutated, helper.UnitState(True, True, True, True, 41))
                helper.write_status(mutated, "prepared")
                with self.assertRaises((helper.ActivationError, helper.ValidationError)):
                    helper.activate(mutated, ctl)
                self.assertNotIn("stop", ctl.events)

    def test_pre_marker_rollback_uses_stopped_snapshot_including_writes_during_stop(self):
        import sqlite3
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        preflight_evidence = helper.database_evidence(self.database)
        stop = ctl.stop
        def stop_after_write():
            if not ctl.disruption_started:
                with sqlite3.connect(self.database) as db:
                    db.execute("INSERT INTO data VALUES (x'beef')")
            stop()
        with mock.patch.object(ctl, "stop", side_effect=stop_after_write), \
             mock.patch.object(helper, "rehearse_current_migration", side_effect=OSError("rehearsal failed")), \
             mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest)), \
             mock.patch.object(helper, "prepare_data_directory"), \
             mock.patch.object(helper, "wait_for_identity") as health:
            self.assertEqual("activation_failed_rolled_back", helper.activate(manifest, ctl))
        stopped_evidence = helper.database_evidence(self.database)
        self.assertNotEqual(preflight_evidence, stopped_evidence)
        self.assertEqual(stopped_evidence, helper.database_evidence(self.transaction / "source.sqlite"))
        self.assertEqual(stopped_evidence[1], helper.verify_snapshot_proof(manifest)["source_digest"])
        with sqlite3.connect(self.database) as db:
            self.assertEqual([(b"\xca\xfe",), (b"\xbe\xef",)], db.execute("SELECT value FROM data ORDER BY rowid").fetchall())
        self.assertEqual("old binary", Path(self.targets.binary).read_text())
        self.assertIn(("restore_state", ctl.previous_state), ctl.events)
        health.assert_called_once_with(manifest, manifest.previous, manifest.previous_health_url)
        self.assertFalse((self.transaction / "migration_started").exists())
        self.assertTrue(all(not path.exists() for path in helper.start_inhibit_paths(manifest)))
        status = helper.retained_status(manifest)
        self.assertEqual("activation_failed_rolled_back", status["state"])
        self.assertFalse(status["migration_started"])

    def test_pre_marker_post_snapshot_mutation_forbids_predecessor_restart(self):
        import sqlite3
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        before = helper.database_evidence(self.database)
        def changed_after_snapshot(_manifest):
            self.assertEqual(before, helper.database_evidence(self.transaction / "source.sqlite"))
            with sqlite3.connect(self.database) as db:
                db.execute("INSERT INTO data VALUES (x'beef')")
            raise OSError("source changed after snapshot")
        with mock.patch.object(helper, "rehearse_current_migration", side_effect=changed_after_snapshot), \
             mock.patch.object(helper, "restore", side_effect=AssertionError("forbidden")) as restore:
            self.assertEqual("activation_failed_rollback_failed", self.execute(manifest, ctl))
        restore.assert_not_called()
        self.assertEqual(before, helper.database_evidence(self.transaction / "source.sqlite"))
        self.assertNotEqual(before, helper.database_evidence(self.database))
        self.assertFalse((self.transaction / "migration_started").exists())
        self.assertFalse(helper.release_claim(manifest))
        self.assert_reboot_inhibited(manifest)

    def test_closed_external_write_during_successful_rehearsal_rejects_live_marker(self):
        import sqlite3
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        before = helper.database_evidence(self.database)
        rehearse = helper.rehearse_current_migration
        def rehearse_then_closed_write(retained):
            rehearse(retained)
            self.assertTrue(json.loads((self.transaction / "rehearsal-proof.json").read_text())["success"])
            with sqlite3.connect(self.database) as db:
                db.execute("INSERT INTO data VALUES (x'beef')")
        commands = []
        runtime = self.runtime(manifest)
        def run(command, **kwargs):
            commands.append(command)
            return runtime(command, **kwargs)
        with mock.patch.object(helper, "rehearse_current_migration", side_effect=rehearse_then_closed_write), \
             mock.patch.object(helper.subprocess, "run", side_effect=run), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
             mock.patch.object(helper, "restore", side_effect=AssertionError("changed predecessor forbidden")) as restore:
            self.assertEqual("activation_failed_rollback_failed", helper.activate(manifest, ctl))
        restore.assert_not_called()
        self.assertNotIn([manifest.targets.binary, "--migrate-only"], commands)
        self.assertIn([manifest.candidate.binary.path, "--migrate-only"], commands)
        self.assertFalse((self.transaction / "migration_started").exists())
        self.assertEqual(before, helper.database_evidence(self.transaction / "source.sqlite"))
        self.assertNotEqual(before, helper.database_evidence(self.database))
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertIn("live source differs from stopped snapshot", status["failure"])
        self.assertFalse(status["migration_started"])
        self.assertFalse(helper.release_claim(manifest))
        self.assertTrue(Path(manifest.active_path).exists())
        self.assert_reboot_inhibited(manifest)
        self.assertTrue(ctl.disruption_started)
        self.assertFalse(any(isinstance(event, tuple) and event[0] in {"restore_state", "start_candidate"} for event in ctl.events))

    def test_pre_marker_restore_requires_unchanged_source(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        with mock.patch.object(helper, "snapshot_current", side_effect=OSError("snapshot failed")), \
             mock.patch.object(helper, "assert_database_exclusive"), \
             mock.patch.object(helper, "prepare_data_directory"), \
             mock.patch.object(helper, "wait_for_identity"):
            self.assertEqual("activation_failed_rolled_back", helper.activate(manifest, ctl))
        self.assertEqual("old binary", Path(self.targets.binary).read_text())
        self.assertTrue(all(not path.exists() for path in helper.start_inhibit_paths(manifest)))
        helper.write_status(manifest, "prepared")
        def changed(*_args):
            self.advance(66)
            raise OSError("source changed")
        with mock.patch.object(helper, "snapshot_current", side_effect=changed), \
             mock.patch.object(helper, "assert_database_exclusive"), \
             mock.patch.object(helper, "restore", side_effect=AssertionError("forbidden")):
            self.assertEqual("activation_failed_rollback_failed", helper.activate(manifest, ctl))
        self.assertFalse(helper.release_claim(manifest))
        self.assert_reboot_inhibited(manifest)

    def test_exclusivity_proves_both_units_and_open_handles(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        with mock.patch.object(helper.subprocess, "run") as run:
            with self.assertRaisesRegex(helper.ActivationError, "both be inactive"):
                helper.assert_database_exclusive(manifest, ctl)
            run.assert_not_called()
        ctl.stop()
        for result in (subprocess.CompletedProcess([], 0, "100\n", ""), subprocess.CompletedProcess([], 1, "", "warning")):
            with mock.patch.object(helper.subprocess, "run", return_value=result):
                with self.assertRaisesRegex(helper.ActivationError, "exclusive"):
                    helper.assert_database_exclusive(manifest, ctl)

    def test_recovery_divergence_status_claim_and_abandon_fail_closed(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        self.assertEqual("current_forward_failed", self.execute(manifest, ctl, "partial"))
        before = Path(manifest.status_path).read_bytes()
        self.advance(81, divergent=True)
        ctl.events.clear()
        with mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest)), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected):
            self.assertEqual("current_forward_failed", helper.recover_current(manifest, ctl))
        self.assertEqual("stop", ctl.events[0])
        self.assertEqual("stop", ctl.events[-1])
        self.assertIn("diverges", json.loads(Path(manifest.status_path).read_text())["failure"])
        with self.assertRaises(helper.ValidationError):
            helper.abandon(manifest)
        for field, bad in (("mode", "ordinary"), ("migration_started", "yes"), ("state", "prepared")):
            status = json.loads(before)
            status[field] = bad
            Path(manifest.status_path).write_text(json.dumps(status))
            ctl.events.clear()
            with mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest)):
                self.assertEqual("current_forward_failed", helper.recover_current(manifest, ctl))
            self.assertEqual("stop", ctl.events[0])
            self.assertEqual("stop", ctl.events[-1])
            self.assertIn("forward-recoverable" if field == "state" else "status",
                          json.loads(Path(manifest.status_path).read_text())["failure"])
            helper.verify_start_inhibit_files(manifest)
        self.assertTrue(Path(manifest.active_path).exists())


    def test_irreversible_marker_fsync_failure_cannot_restore_predecessor(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        original = helper.atomic_write
        def write(path, *args, **kwargs):
            original(path, *args, **kwargs)
            if path.name == "migration_started":
                raise OSError("directory sync completion uncertain")
        with mock.patch.object(helper, "atomic_write", side_effect=write), \
             mock.patch.object(helper.subprocess, "run", side_effect=self.runtime(manifest)) as run, \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
             mock.patch.object(helper, "restore", side_effect=AssertionError("forbidden")):
            self.assertEqual("current_forward_failed", helper.activate(manifest, ctl))
        self.assertFalse(any(call.args[0] == [manifest.targets.binary, "--migrate-only"] for call in run.call_args_list))
        self.assertFalse(helper.release_claim(manifest))

    def test_mode_and_marker_status_cannot_release_claim_or_redeploy(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        self.assertEqual("current_forward_failed", self.execute(manifest, ctl, "partial"))
        for changes in ({"mode": "ordinary", "state": "committed", "migration_started": False},
                        {"state": "precondition_failed", "migration_started": False},
                        {"mode": "unknown", "state": "committed"}):
            with self.subTest(changes=changes):
                helper.write_status(manifest, "current_forward_failed")
                status = json.loads(Path(manifest.status_path).read_text())
                status.update(changes)
                Path(manifest.status_path).write_text(json.dumps(status))
                self.assertFalse(helper.release_claim(manifest))
                with self.assertRaises(helper.ConcurrentDeploy):
                    helper.acquire_claim("f" * 32, self.policy)
        self.assertTrue(Path(manifest.active_path).exists())

    def test_indented_quoted_environment_override_rejects_noncanonical_database_before_stop(self):
        manifest = self.protected()
        original_service = Path(self.targets.service).read_text()
        other = self.database.parent / "other.db"
        for whitespace in (" ", "\t", "  \t"):
            service = original_service + f'{whitespace}Environment="PHOENIX_DB_PATH={other}"  \t\n'
            for name, content in (("service", service), ("environment", "SECRET=installed\n")):
                Path(getattr(self.targets, name)).write_text(content)
                for group in ("candidate", "rollback"):
                    artifact = self.raw[group][name]
                    Path(artifact["path"]).write_text(content)
                    artifact["sha256"] = digest(Path(artifact["path"]))
            self.write_manifest()
            manifest = self.manifest()
            helper.write_status(manifest, "prepared")
            ctl = ProtectedSystemctl(manifest, helper.UnitState(False, True, False, True, 0))
            with self.subTest(whitespace=whitespace), mock.patch.object(helper.subprocess, "run") as run:
                helper.validate_manifest(self.manifest_path, manifest, self.policy)
                with self.assertRaisesRegex(helper.ActivationError, "effective database path drift"):
                    helper.verify_preserved_configuration(manifest)
                with self.assertRaisesRegex(helper.ActivationError, "effective database path drift"):
                    helper.activate(manifest, ctl)
                run.assert_not_called()
            self.assertNotIn("stop", ctl.events)
            self.assertFalse((self.transaction / "migration_started").exists())
        # EnvironmentFile values override even a quoted, indented unit assignment.
        content = f'SECRET=installed\nPHOENIX_DB_PATH={self.database}\n'
        Path(self.targets.environment).write_text(content)
        for group in ("candidate", "rollback"):
            artifact = self.raw[group]["environment"]
            Path(artifact["path"]).write_text(content)
            artifact["sha256"] = digest(Path(artifact["path"]))
        self.write_manifest()
        manifest = self.manifest()
        self.assertEqual(self.database, helper.effective_database(manifest))
        self.assertEqual(self.database, helper.effective_database(manifest, installed=True))
        helper.verify_preserved_configuration(manifest)

    def test_protected_cli_precondition_failure_releases_only_undisrupted_matching_claim(self):
        import sqlite3
        manifest = self.protected()
        for failure in ("source-drift", "reservation", "foreign-claim", "marker"):
            Path(manifest.active_path).write_text(manifest.transaction_id + "\n")
            helper.write_status(manifest, "prepared")
            if failure == "source-drift":
                self.advance(66)
            ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
            def reservation_failed(*_args):
                if failure == "foreign-claim":
                    Path(manifest.active_path).write_text("f" * 32 + "\n")
                elif failure == "marker":
                    (self.transaction / "migration_started").symlink_to(self.transaction / "missing-marker-target")
                raise OSError("reservation failed")
            with self.subTest(failure=failure), \
                 mock.patch.object(helper.os, "geteuid", return_value=0), \
                 mock.patch.object(helper.ValidationPolicy, "production", return_value=self.policy), \
                 mock.patch.object(helper, "Systemctl", return_value=ctl), \
                 mock.patch.object(helper, "prepare_installs", side_effect=reservation_failed), \
                 mock.patch.object(helper.sys, "argv", ["helper.py", "activate", "--manifest", str(self.manifest_path)]):
                self.assertEqual(1, helper.main())
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("precondition_failed", status["state"])
            self.assertEqual(failure == "marker", status["migration_started"])
            self.assertIn("prefix65" if failure == "source-drift" else "reservation failed", status["failure"])
            self.assertEqual(failure in {"foreign-claim", "marker"}, Path(manifest.active_path).exists())
            if failure == "foreign-claim":
                self.assertEqual("f" * 32, Path(manifest.active_path).read_text().strip())
            self.assertNotIn("stop", ctl.events)
            self.assertEqual(failure == "marker", helper.migration_marker_present(self.transaction))
            (self.transaction / "migration_started").unlink(missing_ok=True)
            self.assertTrue(self.manifest_path.exists())
            self.assertTrue(Path(manifest.rollback.binary.path).exists())
            with sqlite3.connect(self.database) as db:
                db.execute("DELETE FROM _migrations WHERE version > 65")

    def test_configuration_and_database_path_drift_are_refused_before_stop(self):
        manifest = self.protected()
        original = Path(self.targets.environment).read_text()
        Path(self.targets.environment).write_text(original + "PHOENIX_DB_PATH=/tmp/other.db\n")
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        with self.assertRaises(helper.ActivationError):
            helper.activate(manifest, ctl)
        self.assertNotIn("stop", ctl.events)
        Path(self.targets.environment).write_text(original)
        manifest = dataclasses.replace(manifest, database_path="/tmp/other.db")
        with self.assertRaisesRegex(helper.ValidationError, "canonical"):
            helper.validate_mode(manifest)

    def test_systemctl_stop_requires_both_units_and_exact_inactive_state(self):
        manifest = self.protected()
        commands = []
        def run(command, **_kwargs):
            commands.append(command)
            if "is-active" in command:
                return subprocess.CompletedProcess(command, 3, "inactive\n", "")
            return subprocess.CompletedProcess(command, 0, "", "")
        ctl = helper.Systemctl(manifest, run=run)
        ctl.stop()
        self.assertEqual(["systemctl", "stop", ctl.service, ctl.socket], commands[0])
        with mock.patch.object(ctl, "property", return_value="activating"):
            with self.assertRaisesRegex(helper.ActivationError, "provably inactive"):
                ctl.assert_inactive()

    def test_loaded_systemd_configuration_drift_is_rejected(self):
        manifest = self.protected()
        ctl = helper.Systemctl(manifest)
        good = {("FragmentPath", ctl.service): self.targets.service,
                ("FragmentPath", ctl.socket): self.targets.socket,
                ("DropInPaths", ctl.service): "", ("DropInPaths", ctl.socket): "",
                ("NeedDaemonReload", ctl.service): "no", ("NeedDaemonReload", ctl.socket): "no",
                ("User", ctl.service): manifest.service_user, ("MainPID", ctl.service): "0"}
        for field in ("FragmentPath", "DropInPaths", "NeedDaemonReload", "User"):
            properties = dict(good)
            properties[field, ctl.service] = "drift"
            with mock.patch.object(ctl, "property", side_effect=lambda unit, name: properties[name, unit]):
                with self.assertRaises(helper.ActivationError):
                    ctl.verify_protected_configuration()
        with mock.patch.object(ctl, "property", side_effect=lambda unit, name: good[name, unit]):
            ctl.verify_protected_configuration()

    def test_retained_manifest_helper_and_registry_are_revalidated(self):
        manifest = self.protected()
        helper.validate_manifest(self.manifest_path, manifest, self.policy)
        for artifact in (manifest.controller_helper, manifest.registry, manifest.candidate.binary):
            path = Path(artifact.path)
            original = path.read_bytes()
            path.write_bytes(original + b"changed")
            with self.assertRaises(helper.ValidationError):
                helper.validate_manifest(self.manifest_path, manifest, self.policy)
            path.write_bytes(original)
        for mode in ("unknown", None, 1):
            with self.assertRaises(helper.ValidationError):
                helper.validate_mode(dataclasses.replace(manifest, mode=mode))
        with self.assertRaises(helper.ValidationError):
            helper.validate_mode(dataclasses.replace(manifest, mode="ordinary"))

    def test_root_handoff_captures_installed_config_and_seals_manifest(self):
        manifest = self.protected()
        root = self.base.resolve() / "root-handoff"
        policy = dataclasses.replace(self.policy, transaction_root=root / "transactions",
                                    status_path=root / "status.json", active_path=root / "active",
                                    activation_lock_path=root / "activation.lock", claim_lock_path=root / "claim.lock")
        destination = policy.transaction_root / manifest.transaction_id
        raw = dataclasses.asdict(manifest)
        raw.update(previous=None, previous_health_url=None, status_path=str(policy.status_path),
                   active_path=str(policy.active_path), activation_lock_path=str(policy.activation_lock_path),
                   claim_lock_path=str(policy.claim_lock_path))
        sources = []
        for name, artifact in (("candidate-binary", manifest.candidate.binary), ("candidate.service", manifest.candidate.service),
                               ("candidate.socket", manifest.candidate.socket), ("candidate.env", manifest.candidate.environment),
                               ("helper.py", manifest.controller_helper), ("migrations.rs", manifest.registry)):
            sources.append({"name": name, "source": artifact.path, "sha256": artifact.sha256})
        for field, filename in (("binary", "candidate-binary"), ("service", "candidate.service"),
                                ("socket", "candidate.socket"), ("environment", "candidate.env")):
            raw["candidate"][field]["path"] = str(destination / filename)
        raw["controller_helper"]["path"] = str(destination / "helper.py")
        raw["registry"]["path"] = str(destination / "migrations.rs")
        source_manifest = self.base / "source-manifest.json"
        source_manifest.write_text(json.dumps(raw))
        sources.append({"name": "manifest.json", "source": str(source_manifest), "sha256": digest(source_manifest)})
        bundle = self.base / "bundle.json"
        bundle.write_text(json.dumps({"transaction_id": manifest.transaction_id, "files": sources}))
        original_copy = helper.copy_handoff_file
        def copy(source, target, expected, _uid, mode):
            original_copy(source, target, expected, os.getuid(), mode)
        with mock.patch.object(helper, "copy_handoff_file", side_effect=copy), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.previous), \
             mock.patch.object(helper, "prepare_data_directory", side_effect=AssertionError("preserve live data ownership")):
            staged_path = helper.stage_handoff(bundle, os.getuid(), policy)
        staged = helper.Manifest.load(staged_path)
        self.assertEqual(0o400, staged_path.stat().st_mode & 0o777)
        self.assertEqual(os.getuid(), staged_path.stat().st_uid)
        self.assertEqual(manifest.previous, staged.previous)
        for field in ("service", "socket", "environment"):
            artifact = getattr(staged.candidate, field)
            self.assertEqual(Path(getattr(self.targets, field)).read_bytes(), Path(artifact.path).read_bytes())
            self.assertEqual(getattr(staged.rollback, field).sha256, artifact.sha256)
        helper.validate_manifest(staged_path, staged, policy)
        self.assertEqual("prepared", helper.retained_status(staged)["state"])


    def test_source_with_same_maximum_but_renamed_or_missing_ledger_is_rejected(self):
        import sqlite3
        manifest = self.protected()
        for mutation in ("UPDATE _migrations SET name='wrong' WHERE version=60", "DELETE FROM _migrations WHERE version=12"):
            with sqlite3.connect(self.database) as db:
                db.execute("DELETE FROM _migrations")
                db.executemany("INSERT INTO _migrations VALUES (?, ?)", self.registry[:65])
                db.execute(mutation)
            helper.write_status(manifest, "prepared")
            ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
            with self.assertRaisesRegex(helper.ActivationError, "prefix65"):
                helper.activate(manifest, ctl)
            self.assertNotIn("stop", ctl.events)

    def test_target_foreign_key_failure_after_invocation_never_restores(self):
        import sqlite3
        manifest = self.protected()
        original = self.runtime(manifest)
        def run(command, **kwargs):
            result = original(command, **kwargs)
            if command == [manifest.targets.binary, "--migrate-only"]:
                with sqlite3.connect(self.database) as db:
                    db.execute("CREATE TABLE parent(id INTEGER PRIMARY KEY)")
                    db.execute("CREATE TABLE child(parent_id INTEGER REFERENCES parent(id))")
                    db.execute("INSERT INTO child VALUES (1)")
            return result
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        with mock.patch.object(helper.subprocess, "run", side_effect=run), \
             mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
             mock.patch.object(helper, "restore", side_effect=AssertionError("forbidden")):
            self.assertEqual("current_forward_failed", helper.activate(manifest, ctl))
        self.assertFalse(helper.release_claim(manifest))
        self.assertEqual(manifest.candidate.binary.sha256, helper.sha256(Path(self.targets.binary)))
        self.assertFalse(any(isinstance(event, tuple) and event[0] == "start_candidate" for event in ctl.events))


    def test_marker_file_and_directory_fsync_complete_before_candidate_spawn(self):
        manifest = self.protected()
        ctl = ProtectedSystemctl(manifest, helper.UnitState(True, True, True, True, 41))
        original_write, original_run = helper.atomic_write, self.runtime(manifest)
        completed = []
        with mock.patch.object(helper.os, "fsync", wraps=helper.os.fsync) as sync:
            def write(path, *args, **kwargs):
                count = sync.call_count
                original_write(path, *args, **kwargs)
                if path.name == "migration_started":
                    self.assertGreaterEqual(sync.call_count - count, 2)
                    completed.append("marker_file_and_directory_synced")
            def run(command, **kwargs):
                if command == [manifest.targets.binary, "--migrate-only"]:
                    self.assertEqual(["marker_file_and_directory_synced"], completed)
                return original_run(command, **kwargs)
            with mock.patch.object(helper, "atomic_write", side_effect=write), \
                 mock.patch.object(helper.subprocess, "run", side_effect=run), \
                 mock.patch.object(helper, "installed_runtime_identity", return_value=manifest.expected), \
                 mock.patch.object(helper, "wait_for_identity"):
                self.assertEqual("committed", helper.activate(manifest, ctl))

if __name__ == "__main__":
    unittest.main()
