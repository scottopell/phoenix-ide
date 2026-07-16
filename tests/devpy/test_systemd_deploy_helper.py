import hashlib
import importlib.util
import json
import os
import pwd
import sys
import tempfile
import unittest
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


if __name__ == "__main__":
    unittest.main()
