import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_harness():
    path = ROOT / "tests/integration/systemd_vm_harness.py"
    spec = importlib.util.spec_from_file_location("systemd_vm_harness_test", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


harness = load_harness()


class ProductionGuardTests(unittest.TestCase):
    def test_accepts_randomized_disposable_resources(self):
        instance = f"{harness.NAME_PREFIX}abc123"
        harness.refuse_production_resources(
            instance,
            f"{harness.NAME_PREFIX}helper-abc123",
            f"/var/tmp/{instance}",
            port=49152,
        )

    def test_rejects_non_qa_instance(self):
        with self.assertRaises(ValueError):
            harness.refuse_production_resources(
                "default",
                f"{harness.NAME_PREFIX}helper-abc123",
                "/var/tmp/default",
            )

    def test_rejects_non_qa_unit(self):
        instance = f"{harness.NAME_PREFIX}abc123"
        with self.assertRaises(ValueError):
            harness.refuse_production_resources(
                instance,
                "phoenix-ide.service",
                f"/var/tmp/{instance}",
            )

    def test_rejects_production_port(self):
        instance = f"{harness.NAME_PREFIX}abc123"
        with self.assertRaises(ValueError):
            harness.refuse_production_resources(
                instance,
                f"{harness.NAME_PREFIX}helper-abc123",
                f"/var/tmp/{instance}",
                port=8031,
            )

    def test_rejects_path_not_bound_to_instance(self):
        instance = f"{harness.NAME_PREFIX}abc123"
        with self.assertRaises(ValueError):
            harness.refuse_production_resources(
                instance,
                f"{harness.NAME_PREFIX}helper-abc123",
                "/opt/phoenix-ide/test",
            )


if __name__ == "__main__":
    unittest.main()
