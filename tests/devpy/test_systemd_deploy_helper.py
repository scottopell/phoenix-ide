import hashlib
import importlib.util
import json
import os
import pwd
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
            "expected": {"version": "2.0.0", "git_sha": "b" * 12},
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

    def test_stage_handoff_copies_allowlisted_files_and_acquires_claim(self):
        sources = []
        for name in ("candidate-binary", "candidate.service", "candidate.socket", "helper.py"):
            source = self.base / f"source-{name}"
            source.write_text(name)
            sources.append({"name": name, "source": str(source), "sha256": digest(source)})
        source_manifest = self.base / "source-manifest.json"
        source_manifest.write_text(json.dumps(self.raw))
        sources.append({"name": "manifest.json", "source": str(source_manifest), "sha256": digest(source_manifest)})
        policy = self.staging_policy()
        with mock.patch.object(helper, "prepare_data_directory"):
            manifest = helper.stage_handoff(self.bundle(sources), os.getuid(), policy)
        self.assertEqual(policy.transaction_root / self.transaction_id / "manifest.json", manifest)
        self.assertEqual(self.transaction_id, policy.active_path.read_text().strip())
        self.assertEqual("candidate-binary", (manifest.parent / "candidate-binary").read_text())
        self.assertEqual(self.raw["expected"], json.loads(manifest.read_text())["expected"])
        status = json.loads(policy.status_path.read_text())
        self.assertEqual("prepared", status["state"])
        self.assertEqual(self.transaction_id, status["transaction_id"])

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


class FakeSystemctl:
    def __init__(self, manifest, previous_state, *, start_failure=None):
        self.manifest = manifest
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
            (self.targets.service, "old service"),
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

    def test_unit_verification_uses_staged_candidate_executable(self):
        manifest = self.manifest()
        calls = []
        service = Path(manifest.candidate.service.path)
        service.write_text(f"[Service]\nExecStart={manifest.targets.binary}\n")

        def run(command, **_kwargs):
            calls.append(command)
            verification_service = Path(command[-1])
            self.assertIn(manifest.candidate.binary.path, verification_service.read_text())
            self.assertNotIn(manifest.targets.binary, verification_service.read_text())
            return subprocess.CompletedProcess(command, 0, "", "")

        helper.Systemctl(manifest, run=run).verify_units(
            Path(manifest.candidate.service.path),
            Path(manifest.candidate.socket.path),
            Path(manifest.candidate.binary.path),
        )
        self.assertEqual("systemd-analyze", calls[0][0])

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
        self.assertEqual("old service", Path(self.targets.service).read_text())
        self.assertEqual("old socket", Path(self.targets.socket).read_text())
        self.assertEqual("OLD=value", Path(self.targets.environment).read_text())
        self.assertEqual("a" * 40, Path(self.targets.deployed_sha).read_text().strip())
        status = json.loads(self.policy.status_path.read_text())
        self.assertEqual("activation_failed_rolled_back", status["state"])
        self.assertEqual("wrong identity", status["failure"])
        self.assertIn(("restore_state", previous_state), controller.events)

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
        self.assertEqual("activation_failed_rollback_failed", state)
        status = json.loads(self.policy.status_path.read_text())
        self.assertEqual("candidate crashed", status["failure"])
        self.assertEqual("rollback start failed", status["rollback_failure"])
        self.assertEqual(manifest.transaction_id, self.policy.active_path.read_text().strip())
        self.assertTrue(helper.status_is_durable_terminal(manifest))
        self.assertFalse(helper.release_claim(manifest))

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


if __name__ == "__main__":
    unittest.main()
