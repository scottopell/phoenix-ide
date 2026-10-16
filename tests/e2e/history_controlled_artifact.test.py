#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
import json
from pathlib import Path
import tempfile
import unittest
import subprocess
import history_controlled_artifact as admission


class ArtifactAdmission(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.app = Path(self.temp.name).resolve()
        for mode, (rate, output, factory, config) in admission.MODES.items():
            for relative in (factory, config, output + "/index.html", output + "/assets/app.js"):
                path = self.app / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(relative)
        guard = self.app / 'ui/qualification/viewReporterBuild.ts'
        guard.parent.mkdir(parents=True, exist_ok=True); guard.write_text('guard')
        (self.app / '.gitignore').write_text('ui/dist*/\nui/*-build.json\n')
        subprocess.run(['git', 'init', '-q', str(self.app)], check=True)
        subprocess.run(['git', '-C', str(self.app), 'add', '.'], check=True)
        subprocess.run(['git', '-C', str(self.app), '-c', 'user.name=test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'fixture'], check=True)
        self.sha = subprocess.check_output(['git', '-C', str(self.app), 'rev-parse', 'HEAD'], text=True).strip()
        self.tree = subprocess.check_output(['git', '-C', str(self.app), 'rev-parse', 'HEAD^{tree}'], text=True).strip()
        self.mode = "controlled_functional"
        self.reset()

    def reset(self):
        rate, output, factory, config = admission.MODES[self.mode]
        self.manifest = self.app / "ui" / ("stock-natural-build.json" if self.mode == "stock_natural" else "controlled-functional-build.json")
        self.receipt = {"schema_version": 1, "evidence_class": self.mode, "sample_rate": rate,
            "source_sha": self.sha, "source_tree": self.tree, "source_dirty": False,
            "guard_relative": "ui/qualification/viewReporterBuild.ts", "guard_sha256": admission.digest(self.app / 'ui/qualification/viewReporterBuild.ts'),
            "admission": 'stock' if self.mode == 'stock_natural' else 'cfgtest_loopback_preview_only', "full_acceptance": False, "overhead_clearance": False, "factory_relative": factory,
            "config_relative": config, "factory_sha256": admission.digest(self.app / factory),
            "config_sha256": admission.digest(self.app / config), "graph_scope": "ui_source_not_dependency_provenance",
            "resolved_module_files": {factory: admission.digest(self.app / factory)},
            "ui_files": {output + '/' + name: admission.digest(self.app / output / name) for name in ["index.html", "assets/app.js"]}}

    def admit(self):
        self.manifest.write_text(json.dumps(self.receipt))
        return admission.admit_artifact(self.app, self.manifest, self.sha, self.tree, self.mode)

    def test_validated_modes_cannot_cross_evidence_boundary(self):
        controlled = self.admit()
        self.assertIs(admission.require_controlled(controlled), controlled)
        with self.assertRaises(ValueError): admission.require_stock(controlled)
        self.mode = "stock_natural"
        self.reset()
        stock = self.admit()
        self.assertIs(admission.require_stock(stock), stock)
        with self.assertRaises(ValueError): admission.require_controlled(stock)
        for raw in [None, {}, "stock_natural"]:
            with self.assertRaises(ValueError): admission.require_stock(raw)
            with self.assertRaises(ValueError): admission.require_controlled(raw)

    def test_rehashed_replacement_assets_cannot_rewrite_independent_build_owner(self):
        self.admit()
        self.mode = 'stock_natural'; self.reset(); self.admit()
        owner = {'ui_artifacts': admission.build_owner_binding(self.app)}
        admission.require_build_owner_binding(self.app, owner)
        self.mode = 'controlled_functional'; self.reset()
        asset = 'ui/dist-controlled-functional/assets/app.js'
        (self.app / asset).write_text('replaced bytes and self-rehashed manifest')
        self.receipt['ui_files'][asset] = admission.digest(self.app / asset)
        self.admit()  # internally consistent is not independent build provenance
        with self.assertRaisesRegex(ValueError, 'build-owner'):
            admission.require_build_owner_binding(self.app, owner)

    def test_identity_and_mode_fields_are_exact(self):
        for key, values in {"schema_version": [True, 2, None], "evidence_class": ["stock_natural", "unknown", None],
                "sample_rate": [True, .25, 0], "source_sha": ["c" * 40], "source_tree": ["d" * 40],
                "factory_relative": [admission.MODES['stock_natural'][2]], "config_relative": ["ui/vite.config.ts"],
                "graph_scope": ["all"]}.items():
            for value in values:
                self.reset(); self.receipt[key] = value
                with self.subTest(key=key, value=value), self.assertRaises(ValueError): self.admit()
        self.reset(); self.receipt['extra'] = True
        with self.assertRaises(ValueError): self.admit()

    def test_missing_hash_extra_asset_and_changed_bytes_rejected(self):
        for key in list(self.receipt):
            self.reset(); del self.receipt[key]
            with self.subTest(key=key), self.assertRaises(ValueError): self.admit()
        self.reset(); self.receipt['ui_files']['ui/dist-controlled-functional/index.html'] = '0' * 64
        with self.assertRaises(ValueError): self.admit()
        self.reset(); (self.app / 'ui/dist-controlled-functional/extra.js').write_text('extra')
        with self.assertRaises(ValueError): self.admit()

    def test_stock_graph_cannot_contain_controlled_factory(self):
        self.mode = 'stock_natural'; self.reset()
        controlled = admission.MODES['controlled_functional'][2]
        self.receipt['resolved_module_files'][controlled] = admission.digest(self.app / controlled)
        with self.assertRaisesRegex(ValueError, 'stock graph'): self.admit()

    def test_controlled_graph_cannot_include_stock_factory(self):
        stock = admission.MODES['stock_natural'][2]
        self.receipt['resolved_module_files'][stock] = admission.digest(self.app / stock)
        with self.assertRaisesRegex(ValueError, 'conflicting stock'): self.admit()

    def test_changed_source_cannot_self_attest_by_rehashing(self):
        factory = self.app / self.receipt['factory_relative']
        factory.write_text('tampered source')
        self.receipt['factory_sha256'] = admission.digest(factory)
        self.receipt['resolved_module_files'][self.receipt['factory_relative']] = admission.digest(factory)
        with self.assertRaisesRegex(ValueError, 'clean pinned Git'): self.admit()

    def test_traversal_symlinks_and_duplicate_manifest_fail(self):
        for name in ['../index.html', '/tmp/file']:
            self.reset(); self.receipt['ui_files'] = {name: '0' * 64}
            with self.assertRaises(ValueError): self.admit()
        self.reset(); target = self.app / 'ui/dist-controlled-functional/assets/app.js'; target.unlink(); target.symlink_to(self.app / 'ui/vite.config.ts')
        with self.assertRaises(ValueError): self.admit()
        self.manifest.write_text('{"schema_version":1,"schema_version":1}')
        with self.assertRaisesRegex(ValueError, 'duplicate'): admission.admit_artifact(self.app,self.manifest,self.sha,self.tree,self.mode)


if __name__ == '__main__': unittest.main()
