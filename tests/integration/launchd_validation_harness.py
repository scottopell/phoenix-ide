#!/usr/bin/env python3
"""Executable disposable validation of the real launchd installer handoff."""

import copy
import hashlib
import importlib.util
import json
import os
import plistlib
import runpy
import shutil
import stat
import sys
import tempfile
from pathlib import Path


EXPECTED_LEGACY_DATABASE_MANIFEST_FIELDS = (
    "database_mode",
    "database_path",
    "database_backup_path",
    "database_backup_sha256",
    "database_backup_verified",
    "database_proof_path",
    "controller_source_commit",
    "controller_helper_sha256",
    "controller_helper_path",
)


ROOT = Path(__file__).resolve().parents[2]
DEV_PATH = ROOT / "dev.py"
SOURCE114 = "23badb8be4bc8d05a7e1c49c5367ceb14ebccd92"


PROVENANCE_PATH = ROOT / ".github" / "artifacts" / "prepared-source114-aarch64.json"


def load_provenance() -> dict:
    value = json.loads(PROVENANCE_PATH.read_text())
    assert value["schema_version"] == 1
    assert value["record_kind"] == "prepared_source114_actions_artifact_provenance"
    return value


def load_dev():
    spec = importlib.util.spec_from_file_location("disposable_launchd_validation_dev", DEV_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def prepared_artifact(expected_names: set[str]) -> Path:
    download = Path(os.environ["PHOENIX_PREPARED_ARTIFACT_ROOT"]).resolve(strict=True)
    entries = list(download.rglob("*"))
    if any(path.is_symlink() for path in entries):
        raise AssertionError("prepared artifact extraction contains a symlink")
    receipts = [
        path
        for path in entries
        if path.name == "PREPARATION-RECEIPT-aarch64-apple-darwin.json"
    ]
    if len(receipts) != 1:
        raise AssertionError(f"expected one prepared receipt, found {len(receipts)}")
    artifact = receipts[0].parent
    expected_files = {
        (artifact / name).resolve(strict=True) for name in expected_names
    }
    actual_files = {path.resolve(strict=True) for path in entries if path.is_file()}
    if actual_files != expected_files:
        raise AssertionError("prepared artifact extraction has unexpected or sibling files")
    for directory in (path for path in entries if path.is_dir()):
        resolved = directory.resolve(strict=True)
        if resolved != artifact and resolved not in artifact.parents:
            raise AssertionError("prepared artifact extraction has an extra sibling directory")
    return artifact


def artifact_witness(root: Path) -> tuple:
    witness = []
    for path in [root, *sorted(root.rglob("*"))]:
        metadata = path.lstat()
        content = (
            os.readlink(path)
            if path.is_symlink()
            else digest(path) if path.is_file() else None
        )
        witness.append((
            str(path.relative_to(root)),
            stat.S_IFMT(metadata.st_mode),
            stat.S_IMODE(metadata.st_mode),
            metadata.st_uid,
            metadata.st_gid,
            content,
        ))
    return tuple(witness)


def check_failed_preparation_cleanup(dev, artifact: Path, download: Path, artifact_before) -> None:
    original_materialize = dev._materialize_helper
    generated_root = None
    unexpected_receipt = None
    injected = RuntimeError("injected disposable helper-materialization failure")

    def fail_after_materializing(
        source_commit: str, destination: Path, source_kind: str
    ) -> None:
        nonlocal generated_root
        generated_root = destination.parents[2]
        assert generated_root.parent == Path(tempfile.gettempdir()).resolve()
        assert generated_root.name.startswith("phoenix-launchd-validation-")
        assert generated_root.is_dir()
        original_materialize(source_commit, destination, source_kind)
        raise injected

    dev._materialize_helper = fail_after_materializing
    try:
        try:
            unexpected_receipt = dev.prepare_disposable_launchd_validation(
                dev.DisposableLaunchdValidationInputs(prepared_artifact=artifact)
            )
        except RuntimeError as exc:
            assert exc is injected, "preparation did not re-raise the materialization failure"
        else:
            raise AssertionError("preparation swallowed the materialization failure")
        assert generated_root is not None, "materialization failure did not reach a generated root"
        assert not generated_root.exists(), "failed preparation leaked its generated root"
    finally:
        dev._materialize_helper = original_materialize
        try:
            if unexpected_receipt is not None:
                shutil.rmtree(unexpected_receipt.root)
        finally:
            try:
                if generated_root is not None and generated_root.exists():
                    shutil.rmtree(generated_root)
            finally:
                assert artifact_witness(download) == artifact_before


def validate_receipt(dev, receipt) -> None:
    assert receipt.candidate_identity.git_sha == SOURCE114
    assert receipt.controller_source_commit != SOURCE114
    assert receipt.candidate_registry_sha256 == dev.PREPARED_SOURCE114_REGISTRY_SHA256
    assert receipt.database_ledger[-1][0] == 114
    assert receipt.root.parent == Path(tempfile.gettempdir()).resolve()
    assert receipt.root.name.startswith("phoenix-launchd-validation-")
    assert receipt.root.stat().st_mode & 0o777 == 0o700
    assert not (receipt.root / "target").exists()
    assert not (receipt.root / "evidence").exists()
    for path in receipt.root.rglob("*"):
        assert receipt.root in path.resolve(strict=False).parents

    manifest = json.loads(receipt.manifest_path.read_text())
    assert manifest["manifest_version"] == dev.LAUNCHD_HANDOFF_PROTOCOL_VERSION
    assert manifest["label"].startswith("test.disposable.phoenix.validation.")
    assert manifest["helper_label"] == f"com.phoenix-ide.deploy.{receipt.transaction_id}"
    assert manifest["source_kind"] == "prepared_artifact"
    assert manifest["source_commit"] == SOURCE114
    assert manifest["current_schema_upgrade"]["prepared_source_baseline"] == 114
    assert (
        manifest["current_schema_upgrade"]["candidate_registry_expected_sha256"]
        == dev.PREPARED_SOURCE114_REGISTRY_SHA256
    )
    assert dev._deploy_manifest_kind(receipt.transaction_path) is dev.DeployManifestKind.CURRENT_SCHEMA

    helper = receipt.transaction_path / "activate.py"
    helper_policy = __import__("runpy").run_path(str(helper))
    loaded = helper_policy["Manifest"].load(receipt.manifest_path)
    helper_policy["validate_pre_disruption_handoff"](loaded)

    original_manifest_bytes = receipt.manifest_path.read_bytes()
    original_manifest_mode = stat.S_IMODE(receipt.manifest_path.stat().st_mode)
    full_root_baseline = dev._disposable_validation_tree_witness(receipt.root)
    manifest_relative = str(receipt.manifest_path.relative_to(receipt.root))
    retained_baseline = tuple(
        item for item in full_root_baseline if item[0] != manifest_relative
    )

    def reject_manifest_bytes(
        wrong: bytes, reason: str, *, reject_classifier: bool = False
    ) -> None:
        try:
            receipt.manifest_path.write_bytes(wrong)
            if reject_classifier:
                assert (
                    dev._deploy_manifest_kind(receipt.transaction_path)
                    is dev.DeployManifestKind.UNKNOWN
                ), f"classifier accepted {reason}"
            try:
                loaded_wrong = helper_policy["Manifest"].load(receipt.manifest_path)
                helper_policy["validate_pre_disruption_handoff"](loaded_wrong)
            except helper_policy["ActivationError"]:
                pass
            else:
                raise AssertionError(f"helper accepted {reason}")
        finally:
            try:
                retained_after = tuple(
                    item
                    for item in dev._disposable_validation_tree_witness(receipt.root)
                    if item[0] != manifest_relative
                )
                assert retained_after == retained_baseline, f"{reason} mutated retained evidence"
            finally:
                receipt.manifest_path.write_bytes(original_manifest_bytes)
                receipt.manifest_path.chmod(original_manifest_mode)
                assert receipt.manifest_path.read_bytes() == original_manifest_bytes
                assert stat.S_IMODE(receipt.manifest_path.stat().st_mode) == original_manifest_mode
                assert (
                    dev._disposable_validation_tree_witness(receipt.root) == full_root_baseline
                ), f"{reason} did not restore the full root"

    def reject_manifest(wrong: dict, reason: str, *, reject_classifier: bool = False) -> None:
        reject_manifest_bytes(
            json.dumps(wrong).encode(), reason, reject_classifier=reject_classifier
        )

    def exercise_plist_negatives(
        *, plist_field: str, sha_field: str, staged_binary_field: str, description: str
    ) -> None:
        plist_path = Path(manifest[plist_field])
        plist_relative = str(plist_path.relative_to(receipt.root))
        original_plist_bytes = plist_path.read_bytes()
        original_plist_mode = stat.S_IMODE(plist_path.stat().st_mode)
        original_plist = plistlib.loads(original_plist_bytes)
        assert "Program" not in original_plist
        assert original_plist["ProgramArguments"] == [manifest["target_binary"]]
        retained_without_manifest_or_plist = tuple(
            item
            for item in full_root_baseline
            if item[0] not in {manifest_relative, plist_relative}
        )

        def reject_plist(wrong: dict, reason: str) -> None:
            try:
                plist_path.write_bytes(plistlib.dumps(wrong))
                wrong_manifest = copy.deepcopy(manifest)
                wrong_manifest[sha_field] = digest(plist_path)
                receipt.manifest_path.write_text(json.dumps(wrong_manifest))
                try:
                    loaded_wrong = helper_policy["Manifest"].load(receipt.manifest_path)
                    helper_policy["validate_pre_disruption_handoff"](loaded_wrong)
                except helper_policy["ActivationError"]:
                    pass
                else:
                    raise AssertionError(f"helper accepted {reason}")
            finally:
                try:
                    retained_after = tuple(
                        item
                        for item in dev._disposable_validation_tree_witness(receipt.root)
                        if item[0] not in {manifest_relative, plist_relative}
                    )
                    assert retained_after == retained_without_manifest_or_plist
                finally:
                    plist_path.write_bytes(original_plist_bytes)
                    plist_path.chmod(original_plist_mode)
                    receipt.manifest_path.write_bytes(original_manifest_bytes)
                    receipt.manifest_path.chmod(original_manifest_mode)
                    assert (
                        dev._disposable_validation_tree_witness(receipt.root)
                        == full_root_baseline
                    )

        program_override = copy.deepcopy(original_plist)
        program_override["Program"] = "/usr/bin/false"
        reject_plist(program_override, f"{description} Program override")
        wrong_target = copy.deepcopy(original_plist)
        wrong_target["ProgramArguments"] = [manifest[staged_binary_field]]
        reject_plist(wrong_target, f"{description} staged-binary program")
        trailing_arguments = copy.deepcopy(original_plist)
        trailing_arguments["ProgramArguments"].append("--unexpected")
        reject_plist(trailing_arguments, f"{description} trailing argument")

    exercise_plist_negatives(
        plist_field="candidate_plist",
        sha_field="candidate_plist_sha256",
        staged_binary_field="candidate_binary",
        description="candidate plist",
    )
    exercise_plist_negatives(
        plist_field="rollback_plist",
        sha_field="rollback_plist_sha256",
        staged_binary_field="rollback_binary",
        description="rollback plist",
    )

    duplicate_cases = {
        "duplicate source_commit": original_manifest_bytes.replace(
            b'"source_commit":',
            b'"source_commit":"0000000000000000000000000000000000000000",'
            b'"source_commit":',
            1,
        ),
        "duplicate transition_timeout_secs": original_manifest_bytes.replace(
            b'"transition_timeout_secs":',
            b'"transition_timeout_secs":31,' b'"transition_timeout_secs":',
            1,
        ),
        "duplicate current_schema_upgrade": original_manifest_bytes.replace(
            b'"current_schema_upgrade":',
            b'"current_schema_upgrade":null,' b'"current_schema_upgrade":',
            1,
        ),
        "duplicate nested prepared_source_baseline": original_manifest_bytes.replace(
            b'"current_schema_upgrade": {',
            b'"current_schema_upgrade": {"prepared_source_baseline":114,',
            1,
        ),
    }
    for reason, raw in duplicate_cases.items():
        assert raw != original_manifest_bytes
        reject_manifest_bytes(raw, reason, reject_classifier=True)

    for wrong_label in (
        "test.disposable.phoenix.validation.helper",
        f"com.phoenix-ide.deploy.{receipt.transaction_id}-wrong",
        f"com.phoenix-ide.deployment.{receipt.transaction_id}",
    ):
        reject_manifest(
            {**manifest, "helper_label": wrong_label}, f"invalid label {wrong_label!r}"
        )

    for wrong_label in ("", "com.example.unrelated-service"):
        reject_manifest(
            {**manifest, "label": wrong_label},
            f"invalid target service label {wrong_label!r}",
        )

    for timeout in ("transition_timeout_secs", "health_timeout_secs"):
        reject_manifest(
            {**manifest, timeout: manifest[timeout] + 1}, f"wrong {timeout}"
        )
        for nonfinite in (float("nan"), float("inf"), float("-inf")):
            reject_manifest(
                {**manifest, timeout: nonfinite}, f"non-finite {timeout}={nonfinite!r}"
            )

    for timeout in ("transition_timeout_secs", "health_timeout_secs"):
        missing_timeout = dict(manifest)
        del missing_timeout[timeout]
        reject_manifest(missing_timeout, f"missing {timeout}")

    assert (
        tuple(helper_policy["LEGACY_DATABASE_MANIFEST_FIELDS"])
        == EXPECTED_LEGACY_DATABASE_MANIFEST_FIELDS
    )
    for legacy_field in EXPECTED_LEGACY_DATABASE_MANIFEST_FIELDS:
        reject_manifest(
            {**manifest, legacy_field: "unexpected legacy evidence"},
            f"legacy top-level field {legacy_field}",
        )

    legacy_manifest = copy.deepcopy(manifest)
    legacy_manifest["current_schema_upgrade"] = None
    legacy_manifest["source_kind"] = "local_head"
    legacy_manifest["source_commit"] = "f" * 40
    legacy_manifest["expected"]["git_sha"] = "f" * 40
    del legacy_manifest["transition_timeout_secs"]
    del legacy_manifest["health_timeout_secs"]
    for legacy_field in EXPECTED_LEGACY_DATABASE_MANIFEST_FIELDS:
        legacy_manifest[legacy_field] = "legacy compatibility evidence"
    try:
        receipt.manifest_path.write_text(json.dumps(legacy_manifest))
        loaded_legacy = helper_policy["Manifest"].load(receipt.manifest_path)
        assert loaded_legacy.current_schema_upgrade is None
        assert (
            loaded_legacy.transition_timeout_secs
            == helper_policy["DEFAULT_TRANSITION_TIMEOUT_SECS"]
        )
        assert (
            loaded_legacy.health_timeout_secs
            == helper_policy["DEFAULT_HEALTH_TIMEOUT_SECS"]
        )
        retained_after = tuple(
            item
            for item in dev._disposable_validation_tree_witness(receipt.root)
            if item[0] != manifest_relative
        )
        assert retained_after == retained_baseline
    finally:
        receipt.manifest_path.write_bytes(original_manifest_bytes)
        receipt.manifest_path.chmod(original_manifest_mode)
        assert dev._disposable_validation_tree_witness(receipt.root) == full_root_baseline

    downgraded_source114 = copy.deepcopy(manifest)
    del downgraded_source114["current_schema_upgrade"]["prepared_source_baseline"]
    del downgraded_source114["current_schema_upgrade"][
        "candidate_registry_expected_sha256"
    ]
    downgraded_source114["transition_timeout_secs"] += 1
    reject_manifest(
        downgraded_source114,
        "prepared source114 identity without both qualification fields",
    )

    paired_fields = (
        "database_path", "backup_path", "proof_path", "controller_source_commit",
        "controller_helper_sha256", "controller_helper_path",
    )
    reject_manifest(
        {
            **manifest,
            "paired_database_upgrade": {
                key: manifest["current_schema_upgrade"][key] for key in paired_fields
            },
        },
        "combined paired and current-schema top-level modes",
        reject_classifier=True,
    )
    reject_manifest(
        {
            **manifest,
            "current_schema_upgrade": {
                **manifest["current_schema_upgrade"], "unexpected": "forbidden"
            },
        },
        "extra prepared-source114 mode evidence",
        reject_classifier=True,
    )
    assert dev._disposable_validation_tree_witness(receipt.root) == full_root_baseline
    assert dev._deploy_manifest_kind(receipt.transaction_path) is dev.DeployManifestKind.CURRENT_SCHEMA
    helper_policy["validate_pre_disruption_handoff"](
        helper_policy["Manifest"].load(receipt.manifest_path)
    )
    assert dev._disposable_validation_tree_witness(receipt.root) == full_root_baseline
    assert not (receipt.root / "target").exists()
    assert not (receipt.root / "evidence").exists()
    assert not (receipt.transaction_path / "database-backup.sqlite3").exists()
    assert not (receipt.transaction_path / "database-proof.json").exists()


def main() -> None:
    provenance = load_provenance()
    dev = load_dev()
    assert SOURCE114 == provenance["candidate"]["source_commit"]
    assert (
        dev.PREPARED_SOURCE114_REGISTRY_SHA256
        == provenance["candidate"]["candidate_registry_sha256"]
    )
    expected_names = {item["name"] for item in provenance["bundle"]["files"]}
    assert expected_names == set(
        dev._prepared_artifact_bundle_names(provenance["candidate"]["target"]).values()
    )
    expected_effects = provenance["expected_validation"]["effects"]
    assert expected_effects == {
        "claim": 0,
        "stop": 0,
        "backup": 0,
        "migrate": 0,
        "publish": 0,
        "activate": 0,
        "cleanup": 0,
    }
    effects = {name: 0 for name in expected_effects}
    helper_sinks = {
        "release_durable_terminal_claim": "claim",
        "release_claim": "claim",
        "Launchctl": "stop",
        "create_database_backup": "backup",
        "run_current_schema_migration": "migrate",
        "reserve_database_capacity": "migrate",
        "prepare_atomic_install": "publish",
        "prepare_plist_publication": "publish",
        "_publish_current_repair": "publish",
        "activate": "activate",
        "activate_current_repair": "activate",
        "cleanup_precondition_capacity": "cleanup",
        "release_capacity_reservation": "cleanup",
        "release_unsnapshotted_capacity": "cleanup",
        "finalize_paired": "cleanup",
        "recover_paired": "cleanup",
        "finalize_verified_predecessor": "cleanup",
    }
    original_run_path = runpy.run_path

    def instrumented_run_path(path, *args, **kwargs):
        policy = original_run_path(path, *args, **kwargs)
        if "validate_pre_disruption_handoff" not in policy:
            return policy
        validator_globals = policy["validate_pre_disruption_handoff"].__globals__
        for sink, effect in helper_sinks.items():
            assert sink in validator_globals

            def forbidden(*_args, _effect=effect, _sink=sink, **_kwargs):
                effects[_effect] += 1
                raise AssertionError(f"validation reached forbidden helper sink {_sink}")

            validator_globals[sink] = forbidden
            policy[sink] = forbidden
            assert policy["validate_pre_disruption_handoff"].__globals__[sink] is forbidden
        return policy

    artifact = prepared_artifact(expected_names)
    download = Path(os.environ["PHOENIX_PREPARED_ARTIFACT_ROOT"]).resolve(strict=True)
    artifact_before = artifact_witness(download)
    receipt = None
    runpy.run_path = instrumented_run_path
    try:
        check_failed_preparation_cleanup(dev, artifact, download, artifact_before)
        receipt = dev.prepare_disposable_launchd_validation(
            dev.DisposableLaunchdValidationInputs(prepared_artifact=artifact)
        )
        assert artifact_witness(download) == artifact_before
        validate_receipt(dev, receipt)
        assert len(receipt.database_ledger) == provenance["expected_validation"][
            "database_ledger_entries"
        ]
        assert effects == expected_effects
    finally:
        runpy.run_path = original_run_path
        try:
            if receipt is not None:
                shutil.rmtree(receipt.root)
        finally:
            assert artifact_witness(download) == artifact_before
    rendered_effects = " ".join(f"{name}={count}" for name, count in effects.items())
    print(
        "launchd-validation-only: PASS "
        f"transaction={receipt.transaction_id} ledger={len(receipt.database_ledger)} "
        f"{rendered_effects}"
    )


if __name__ == "__main__":
    main()
