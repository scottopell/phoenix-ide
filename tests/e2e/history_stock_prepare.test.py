#!/usr/bin/env python3
"""Static workflow/receipt tests; never compile, serve, browse, or seed a database."""
import ast
from contextlib import chdir
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import textwrap
import types
import unittest
from unittest.mock import Mock, patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import history_controlled_artifact as artifact

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = (ROOT / '.github/workflows/ci.yml').read_text()
LANE = WORKFLOW.split('  qualify-history-realserver:\n', 1)[1].split('\n  check-ui:', 1)[0]
BRANCH = 'prepare/exact-f16-stock-admission'


def step(prefix):
    return next(value for value in LANE.split('      - name: ')[1:] if value.startswith(prefix))


def python_block(prefix):
    value = step(prefix).split("          python3 - <<'PY'\n", 1)[1].split('\n          PY', 1)[0]
    return textwrap.dedent(value)


class StockPrepareTests(unittest.TestCase):
    def test_exact_pin_and_parent_review_branch_are_narrow(self):
        self.assertIn("github.head_ref == '" + BRANCH + "'", LANE)
        self.assertIn("STOCK_PREPARE_ONLY: ${{ github.head_ref == '" + BRANCH + "' }}", LANE)
        self.assertIn('ref: f16afb227d9336a863c0b0345960ea9a6d62b7c7', LANE)
        self.assertIn('369305dcd94862d4ee12e7af9fd463d5da14d808', LANE)
        self.assertIn('persist-credentials: false', LANE)

    def test_stock_compilation_and_receipt_precede_controlled_build(self):
        names = ['Build stock UI before', 'Genuine typed-contract codegen',
                 'Verify and retain instrumented libtest', 'Retain stock receiving libtest',
                 'Build separate controlled UI and bind']
        positions = [LANE.index('      - name: ' + name) for name in names]
        self.assertEqual(positions, sorted(positions))
        stock = step(names[0])
        self.assertIn('node node_modules/vite/bin/vite.js build\n', stock)
        self.assertNotIn('--mode', stock)
        self.assertNotIn('controlled-functional.config', stock)
        self.assertIn("selector = ['cargo', 'test', '--no-run', '--message-format=json']", LANE)
        self.assertIn("'compiler_target_features':", LANE)
        self.assertIn("'compiler_target_profile':", LANE)
        self.assertIn("'build_configuration_files':", LANE)

    def test_prepare_branch_runs_existing_stock_cli_once_with_required_chrome(self):
        chrome = step('Prepare owned Chromium')
        self.assertIn('playwright install --with-deps chromium', chrome)
        self.assertNotIn('if:', chrome)
        native = step('Exact Linux owned Chrome')
        self.assertIn("if: ${{ env.STOCK_PREPARE_ONLY != 'true' }}", native)
        stock = step('Stock joint fixture admission and bounded natural sampling')
        self.assertIn("if: ${{ env.STOCK_PREPARE_ONLY == 'true' }}", stock)
        self.assertIn('working-directory: harness', stock)
        self.assertIn('--joint-telemetry \\', stock)
        self.assertNotIn('--controlled-functional', stock)
        self.assertIn('--app-root "$GITHUB_WORKSPACE/pinned-app"', stock)
        self.assertIn('--instrumented-exe "$GITHUB_WORKSPACE/opening-stock-libtest"', stock)
        self.assertIn('--instrumented-receipt "$GITHUB_WORKSPACE/opening-stock-instrumented-receipt.json"', stock)
        self.assertIn('--output "$GITHUB_WORKSPACE/opening-stock-evidence" --timeout 500', stock)
        self.assertEqual(stock.count('tests/e2e/history_opening_browser.py \\'), 1)
        self.assertEqual(LANE.count('tests/e2e/history_opening_browser.py \\'), 2)
        self.assertIn("if: ${{ env.STOCK_PREPARE_ONLY != 'true' }}", step('Controlled-functional six-report'))
        self.assertLess(LANE.index('Full UI source qualification'), LANE.index('Stock joint fixture admission'))
        self.assertNotIn('--fixture-admission-only', LANE)
        self.assertNotIn('blocked_existing_driver', LANE)
        self.assertNotIn('opening-stock-admission-gate', LANE)
        driver = (ROOT / 'tests/e2e/history_opening_browser.py').read_text()
        self.assertIn('"stock-natural-build.json"', driver.replace("'", '"'))
        self.assertIn('("ordinary", "global")', driver)
        self.assertIn('fixtures = joint_fixture(', driver)
        self.assertIn('case / "telemetry-fixture.json"', driver)
        self.assertIn('app / "ui/scripts/qualify-view-telemetry.mjs"', driver)
        self.assertIn('sink_path = e2e / "view_telemetry_sink.py"', driver)
        joint = (ROOT / 'ui/scripts/history-joint-telemetry.mjs').read_text()
        self.assertIn('runTelemetryChecks', joint)

    def test_pinned_sampling_deadline_and_module_hashes_are_unchanged(self):
        import subprocess
        driver = ast.parse((ROOT / 'tests/e2e/history_opening_browser.py').read_text())
        pins = next(ast.literal_eval(node.value) for node in driver.body if isinstance(node, ast.Assign)
                    and any(isinstance(target, ast.Name) and target.id == 'TELEMETRY_MODULES' for target in node.targets))
        source = None
        for name, expected in pins.items():
            data = subprocess.check_output(['git', '-C', str(Path(os.environ.get('APP_ROOT', ROOT))), 'show',
                                            'f16afb227d9336a863c0b0345960ea9a6d62b7c7:' + name])
            self.assertEqual(hashlib.sha256(data).hexdigest(), expected)
            if name == 'ui/scripts/qualify-view-telemetry.mjs':
                source = data.decode()
        self.assertIn('const outerDeadline = Date.now() + 285_000;', source)
        self.assertIn('report.sample_rate, 0.25', source)
        self.assertIn("5000, 'outer context cleanup'", source)
        self.assertIn("5000, 'outer operation drain'", source)
        self.assertLess(285 + 5 + 5, 300)

    def test_pinned_source_lookups_use_separate_app_authority_or_local_fallback(self):
        harness = self.out / 'shallow-harness'
        app = self.out / 'full-history-app'
        harness.mkdir()
        app.mkdir()
        pin = 'f16afb227d9336a863c0b0345960ea9a6d62b7c7'
        cases = (
            ('history_stock_prepare.test.py',
             'test_pinned_sampling_deadline_and_module_hashes_are_unchanged',
             'ui/scripts/qualify-view-telemetry.mjs', {}),
            ('history_opening_browser.test.py',
             'test_ui_command_is_accepted_by_exact_app_lane_resolver_not_ci_group_alias',
             'dev.py', {'text': True}),
        )
        for filename, method, relative, kwargs in cases:
            tree = ast.parse((ROOT / 'tests/e2e' / filename).read_text())
            function = next(node for node in ast.walk(tree)
                            if isinstance(node, ast.FunctionDef) and node.name == method)
            lookup = next(node for node in ast.walk(function)
                          if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                          and node.func.attr == 'check_output')
            expression = compile(ast.Expression(lookup), filename, 'eval')
            for env, authority in (({'APP_ROOT': str(app)}, app), ({}, harness)):
                with self.subTest(filename=filename, env=env), \
                        patch.dict(os.environ, env, clear=True), \
                        patch.object(subprocess, 'check_output', return_value=b'UNIT ONLY source') as git_show:
                    eval(expression, {'subprocess': subprocess, 'Path': Path, 'os': os,
                                      'ROOT': harness, 'name': relative,
                                      'opening': types.SimpleNamespace(ROOT=harness, PIN=pin)})
                    git_show.assert_called_once_with(
                        ['git', '-C', str(authority), 'show', pin + ':' + relative], **kwargs)

    def test_both_artifacts_and_failure_receipts_are_retained(self):
        upload = step('Retain stock artifacts and safe publication')
        self.assertIn("if: ${{ always() && env.STOCK_PREPARE_ONLY == 'true' }}", upload)
        for name in ('pinned-app/ui/dist/', 'pinned-app/ui/stock-natural-build.json',
                     'opening-stock-libtest', 'opening-stock-receiving-build.json',
                     'opening-stock-instrumented-receipt.json', 'opening-stock-publication.json',
                     'opening-instrumented-selector.log', 'opening-ui-build-owner.json'):
            self.assertIn('\n            ' + name + '\n', upload)
        self.assertNotIn('opening-stock-evidence/', upload)
        self.assertNotIn('opening-browser-evidence/', upload)
        self.assertNotIn('opening-controlled-publication.json', LANE)
        publish = step('Publish only stock allowlisted synthetic runtime distributions')
        self.assertIn("if: ${{ always() && env.STOCK_PREPARE_ONLY == 'true' }}", publish)
        self.assertEqual(publish.count('python3 -B tests/e2e/history_runtime_publication.py'), 1)
        controlled = step('Retain receipts on success or failure')
        self.assertIn("if: ${{ always() && env.STOCK_PREPARE_ONLY != 'true' }}", controlled)
        import subprocess
        original = subprocess.check_output(['git', '-C', str(ROOT), 'show', 'HEAD:.github/workflows/ci.yml'], text=True)
        original = original.split('      - name: Retain receipts on success or failure', 1)[1].split('\n  check-ui:', 1)[0].split('      - name:', 1)[0]
        self.assertEqual(controlled.split('          path: |', 1)[1].rstrip(),
                         original.split('          path: |', 1)[1].rstrip())

    def test_all_inline_python_is_syntactically_valid(self):
        blocks = re.findall(r"          python3 -[^\n]* <<'PY'\n(.*?)\n          PY", LANE, re.S)
        self.assertGreaterEqual(len(blocks), 7)
        for block in blocks:
            ast.parse(textwrap.dedent(block))

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.out = Path(self.temp.name)
        self.app = self.out / 'pinned-app'
        (self.app / 'ui/dist').mkdir(parents=True)
        (self.out / 'opening-instrumented-libtest').write_bytes(b'UNIT ONLY libtest bytes')
        self.sha = hashlib.sha256((self.out / 'opening-instrumented-libtest').read_bytes()).hexdigest()
        self.stock = artifact.StockArtifact(self.app / 'ui/dist', 'a' * 64, (('index.html', 'b' * 64),))
        self.receipt = {'app_source': 'f16afb227d9336a863c0b0345960ea9a6d62b7c7',
                        'app_tree': '369305dcd94862d4ee12e7af9fd463d5da14d808',
                        'executable_sha256': self.sha, 'ui_files': dict(self.stock.ui_files),
                        'evidence_class': 'instrumented_cfg_test_library_not_shipping_runtime'}
        (self.out / 'opening-instrumented-receipt.json').write_text(json.dumps(self.receipt))

    def retain(self):
        fake = types.ModuleType('history_controlled_artifact')
        fake.admit_artifact = Mock(return_value=self.stock)
        fake.require_stock = artifact.require_stock
        with chdir(self.app), patch.dict(sys.modules, {'history_controlled_artifact': fake}):
            exec(compile(python_block('Retain stock receiving libtest'), 'actual-stock-retain', 'exec'), {})
        return json.loads((self.out / 'opening-stock-receiving-build.json').read_text())

    def test_stock_receipt_is_instrumented_unexecuted_and_byte_bound(self):
        value = self.retain()
        self.assertEqual((self.out / 'opening-stock-libtest').read_bytes(), b'UNIT ONLY libtest bytes')
        self.assertEqual(value['executable_sha256'], self.sha)
        self.assertEqual(value['stock_manifest_sha256'], self.stock.manifest_sha256)
        self.assertEqual(value['receiving_ui_evidence_class'], 'stock_natural')
        self.assertEqual(value['evidence_class'], 'instrumented_cfg_test_library_not_shipping_runtime')
        self.assertEqual(value['runtime_admission'], 'not_executed')
        self.assertIs(value['full_acceptance'], False)
        self.assertIs(value['overhead_clearance'], False)

    def test_retain_rejects_controlled_before_stock_or_changed_binary(self):
        controlled = self.app / 'ui/controlled-functional-build.json'
        controlled.write_text('UNIT ONLY marker')
        with self.assertRaisesRegex(AssertionError, 'precede controlled'):
            self.retain()
        controlled.unlink()
        (self.out / 'opening-instrumented-libtest').write_bytes(b'UNIT ONLY replacement')
        with self.assertRaises(AssertionError):
            self.retain()
        self.assertFalse((self.out / 'opening-stock-receiving-build.json').exists())

    def bind(self):
        owner = {'app_source': self.receipt['app_source'], 'app_tree': self.receipt['app_tree'],
                 'ui_artifacts': {'stock_natural': {'manifest_sha256': self.stock.manifest_sha256},
                                  'controlled_functional': {'manifest_sha256': 'c' * 64}}}
        (self.out / 'opening-ui-build-owner.json').write_text(json.dumps(owner))
        code = python_block('Build separate controlled UI and bind')
        tail = code[code.index("receipt=json.loads((root.parent/"):]
        exec(compile(tail, 'actual-two-build-binding', 'exec'),
             {'root': self.app, 'json': json, 'module': artifact,
              'source': self.receipt['app_source'], 'tree': self.receipt['app_tree']})
        return owner

    def test_bound_driver_receipt_keeps_real_two_build_binding_without_fake_controlled_mode(self):
        self.retain()
        owner = self.bind()
        value = json.loads((self.out / 'opening-stock-instrumented-receipt.json').read_text())
        self.assertEqual(value['ui_artifacts'], owner['ui_artifacts'])
        self.assertEqual(value['executable_sha256'], self.sha)
        self.assertEqual(value['receiving_ui_evidence_class'], 'stock_natural')
        self.assertEqual(value['evidence_class'], 'instrumented_cfg_test_library_not_shipping_runtime')
        self.assertNotIn('controlled_functional', value.get('evidence_class', ''))
        self.assertFalse((self.out / 'opening-stock-admission-gate.json').exists())

    def test_binding_rejects_replaced_stock_executable_manifest_or_assets(self):
        self.retain()
        stock_path = self.out / 'opening-stock-receiving-build.json'
        original = stock_path.read_text()
        for key, replacement in (('executable_sha256', 'd' * 64), ('stock_manifest_sha256', 'e' * 64),
                                 ('stock_ui_files', {'index.html': 'f' * 64})):
            value = json.loads(original)
            value[key] = replacement
            stock_path.write_text(json.dumps(value))
            with self.subTest(key=key), self.assertRaises(AssertionError):
                self.bind()
            self.assertFalse((self.out / 'opening-stock-instrumented-receipt.json').exists())
        stock_path.write_text(original)
        (self.out / 'opening-stock-libtest').write_bytes(b'UNIT ONLY changed stock')
        with self.assertRaises(AssertionError):
            self.bind()


if __name__ == '__main__':
    unittest.main()
