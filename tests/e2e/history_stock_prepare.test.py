#!/usr/bin/env python3
"""Static workflow/receipt tests; never compile, serve, browse, or seed a database."""
import ast
from contextlib import chdir
import hashlib
import json
import os
import posixpath
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


def geometry_source_preflight(repository, selected_head):
    """Inspect selected Git blobs only; no capture imports, install, or execution."""
    def git(*args):
        return subprocess.check_output(['git', '-C', str(repository), *args])

    head = git('rev-parse', selected_head + '^{commit}').decode().strip()
    entries = {}
    for entry in git('ls-tree', '-rz', head).split(b'\0'):
        if entry:
            metadata, name = entry.split(b'\t', 1)
            entries[name.decode()] = metadata.decode().split()
    blobs = {}

    def read(name):
        assert name in entries, f'geometry source missing at selected head: {name}'
        mode, kind, oid = entries[name]
        assert kind == 'blob' and mode in ('100644', '100755'), name
        if name not in blobs:
            blobs[name] = git('cat-file', 'blob', oid)
        return blobs[name].decode()

    workflow = read('.github/workflows/ci.yml')
    geometry = workflow.split('\n  check-finalization-geometry:\n', 1)[1].split('\n  check-fast:', 1)[0]
    assert "    if: ${{ needs.plan_check.outputs.ui == 'true' }}" in geometry
    assert geometry.count('uses: actions/checkout@') == 1
    assert 'ref: ${{ github.event.pull_request.head.sha || github.sha }}' in geometry
    assert 'git rev-parse HEAD > ui/qa-artifacts/assistant-finalization/source.txt' in geometry
    assert not re.search(r'(?m)^\s+(?:cp|curl|wget|git (?:show|checkout|restore))\s', geometry)
    captures = re.findall(r'^          node (scripts/[^\s]+)$', geometry, re.M)
    assert captures == [
        'scripts/capture-assistant-finalization.mjs',
        'scripts/capture-streaming-table.mjs',
        'scripts/capture-assistant-finalization-transcript.mjs',
        'scripts/capture-subagent-task-details.mjs',
    ], captures
    assert geometry.index('\n          cd ui\n') < geometry.index('\n          node ')
    hash_line = re.findall(r'^          sha256sum (.+) > ui/qa-artifacts/assistant-finalization/source-files.sha256$', geometry, re.M)
    assert len(hash_line) == 1
    hashed_paths = hash_line[0].split()
    assert hashed_paths == ['ui/src/index.css', 'ui/src/components/StreamingMessage.tsx',
                            'ui/src/components/MessageComponents.tsx']
    for name in hashed_paths:
        read(name)

    def relative(owner, reference):
        name = posixpath.normpath(posixpath.join(posixpath.dirname(owner), reference.split('?', 1)[0]))
        candidates = [name] if posixpath.splitext(name)[1] else [name + suffix for suffix in ('.tsx', '.ts', '.mjs')]
        matches = [candidate for candidate in candidates if candidate in entries]
        assert len(matches) == 1, f'geometry relative resource missing/ambiguous: {owner}: {reference}'
        return matches[0]

    pending = []
    for capture in captures:
        name = 'ui/' + capture
        source = read(name)
        story = capture.removeprefix('scripts/capture-').removesuffix('.mjs')
        assert story + '--' in source, name
        pending.append('ui/src/stories/' + story + '.stories.tsx')
        for reference in re.findall(r"new URL\(['\"]([^'\"]+)['\"], import\.meta\.url\)", source):
            read(relative(name, reference))
    visited = set()
    while pending:
        name = pending.pop()
        if name in visited:
            continue
        visited.add(name)
        source = read(name)
        for reference in re.findall(r"(?:from\s+|import\s*)['\"](\.[^'\"]+)['\"]", source):
            dependency = relative(name, reference)
            read(dependency)
            if dependency.startswith('ui/src/fixtures/') and dependency.endswith(('.tsx', '.ts')):
                pending.append(dependency)
    for name in ('ui/src/components/AssistantMessageChrome.tsx',
                 'ui/src/components/AssistantMessageChrome.css',
                 'ui/src/components/SubAgentTaskDetails.tsx',
                 'ui/src/components/SubAgentTaskDetails.css'):
        read(name)
    for job in ('notify-main-failure', 'close-main-failure'):
        body = re.split(r'\n  (?=\S)', workflow.split('\n  ' + job + ':\n', 1)[1], maxsplit=1)[0]
        needs = re.search(r'^    needs: \[([^\]]+)\]$', body, re.M)
        assert needs and 'check-finalization-geometry' in needs.group(1).split(', '), job
    return head, blobs, {name: hashlib.sha256(blobs[name]).hexdigest() for name in hashed_paths}


class StockPrepareTests(unittest.TestCase):
    def test_exact_pin_and_parent_review_branch_are_narrow(self):
        self.assertIn("github.head_ref == '" + BRANCH + "'", LANE)
        self.assertIn("STOCK_PREPARE_ONLY: ${{ github.head_ref == '" + BRANCH + "' }}", LANE)
        self.assertIn('ref: f16afb227d9336a863c0b0345960ea9a6d62b7c7', LANE)
        self.assertIn('369305dcd94862d4ee12e7af9fd463d5da14d808', LANE)
        self.assertIn('persist-credentials: false', LANE)

    def test_offline_label_excludes_stock_runtime_and_probes_require_separate_opt_in(self):
        guard = next(line.strip() for line in LANE.splitlines() if line.startswith('    if:'))
        self.assertEqual(guard, "if: ${{ github.event_name == 'pull_request' && "
                         "github.event.pull_request.head.repo.full_name == github.repository && "
                         "(github.head_ref == 'qualification/continuation-opening-codegen' || "
                         "github.head_ref == 'integration/v013-history-current-main' || "
                         "github.head_ref == 'prepare/exact-f16-stock-admission') && "
                         "!contains(github.event.pull_request.labels.*.name, 'offline-regression-only') }}")
        probe = WORKFLOW.split('  context-close-functional-probe:\n', 1)[1].split('\n  reporter-pagehide-functional-probe:', 1)[0]
        self.assertEqual(next(line.strip() for line in probe.splitlines() if line.startswith('    if:')),
                         "if: ${{ github.event_name == 'pull_request' && "
                         "github.event.pull_request.head.repo.full_name == github.repository && "
                         "github.head_ref == 'prepare/exact-f16-stock-admission' && "
                         "contains(github.event.pull_request.labels.*.name, 'offline-regression-only') && "
                         "contains(github.event.pull_request.labels.*.name, 'context-close-probe') }}")
        outside_runtime = WORKFLOW.replace('  qualify-history-realserver:\n' + LANE, '')
        outside_owned_jobs = outside_runtime.replace('  context-close-functional-probe:\n' + probe, '')
        reporter = WORKFLOW.split('  reporter-pagehide-functional-probe:\n', 1)[1].split('\n  qualify-history-realserver:', 1)[0]
        outside_owned_jobs = outside_owned_jobs.replace('  reporter-pagehide-functional-probe:\n' + reporter, '')
        self.assertNotIn('offline-regression-only', outside_owned_jobs)
        triggers = WORKFLOW.split('\non:\n', 1)[1].split('\nconcurrency:', 1)[0]
        self.assertIn('  pull_request:\n', triggers)
        self.assertNotIn('types:', triggers)

    def test_geometry_capture_resources_exist_in_committed_head(self):
        head, blobs, hashes = geometry_source_preflight(ROOT, 'HEAD')
        self.assertEqual(head, subprocess.check_output(
            ['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip())
        self.assertEqual(len([name for name in blobs if name.startswith('ui/scripts/capture-')]), 4)
        self.assertIn('ui/src/fixtures/assistantFinalization/acl-reply.md', blobs)
        self.assertEqual(len(hashes), 3)

    def geometry_repository(self):
        head, blobs, hashes = geometry_source_preflight(ROOT, 'HEAD')
        repo = self.out / 'geometry-source'
        repo.mkdir()
        subprocess.run(['git', 'init', '-q', str(repo)], check=True)
        for name, data in blobs.items():
            path = repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True)
        self.commit_geometry(repo)
        return repo, blobs, hashes

    def commit_geometry(self, repo):
        subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Source Test',
                        '-c', 'user.email=source-test@example.invalid',
                        'commit', '-qm', 'UNIT ONLY geometry source'], check=True)

    def test_geometry_selected_head_ignores_ambient_files_and_hashes_git_blobs(self):
        repo, blobs, hashes = self.geometry_repository()
        selected = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip()
        for name in blobs:
            (repo / name).write_bytes(b'UNIT ONLY dirty ambient bytes')
        actual_head, actual_blobs, actual_hashes = geometry_source_preflight(repo, selected)
        self.assertEqual(actual_head, selected)
        self.assertEqual(actual_blobs, blobs)
        self.assertEqual(actual_hashes, hashes)

    def test_geometry_missing_entrypoint_fails_before_install_even_with_ambient_copy(self):
        repo, blobs, _ = self.geometry_repository()
        name = 'ui/scripts/capture-streaming-table.mjs'
        subprocess.run(['git', '-C', str(repo), 'rm', '-q', name], check=True)
        self.commit_geometry(repo)
        (repo / name).write_bytes(blobs[name])
        install = Mock()
        with self.assertRaisesRegex(AssertionError, 'missing at selected head: ' + re.escape(name)):
            geometry_source_preflight(repo, 'HEAD')
            install()
        install.assert_not_called()
        # Selecting the preceding complete commit is independent of ambient HEAD.
        geometry_source_preflight(repo, 'HEAD^')

    def test_geometry_missing_relative_fixture_and_story_fail_closed(self):
        repo, blobs, _ = self.geometry_repository()
        for name in ('ui/src/fixtures/assistantFinalization/acl-reply.md',
                     'ui/src/stories/subagent-task-details.stories.tsx'):
            subprocess.run(['git', '-C', str(repo), 'rm', '-q', name], check=True)
            self.commit_geometry(repo)
            (repo / name).write_bytes(blobs[name])
            with self.subTest(name=name), self.assertRaisesRegex(AssertionError, 'geometry .*missing'):
                geometry_source_preflight(repo, 'HEAD')
            subprocess.run(['git', '-C', str(repo), 'add', name], check=True)
            self.commit_geometry(repo)

    def test_geometry_workflow_source_binding_and_both_aggregators_fail_closed(self):
        repo, blobs, _ = self.geometry_repository()
        path = repo / '.github/workflows/ci.yml'
        workflow = blobs['.github/workflows/ci.yml'].decode()
        geometry_start = workflow.index('\n  check-finalization-geometry:\n')
        mutations = [
            ('ref: ${{ github.event.pull_request.head.sha || github.sha }}', 'ref: main'),
            ('git rev-parse HEAD > ui/qa-artifacts/assistant-finalization/source.txt',
             'echo main > ui/qa-artifacts/assistant-finalization/source.txt'),
            ('sha256sum ui/src/index.css ', 'sha256sum ui/src/missing.css '),
            ('          node scripts/capture-streaming-table.mjs\n', ''),
        ]
        for before, after in mutations:
            changed = workflow[:geometry_start] + workflow[geometry_start:].replace(before, after, 1)
            path.write_text(changed)
            subprocess.run(['git', '-C', str(repo), 'add', '.github/workflows/ci.yml'], check=True)
            self.commit_geometry(repo)
            with self.subTest(mutation=before), self.assertRaises(AssertionError):
                geometry_source_preflight(repo, 'HEAD')
        for job in ('notify-main-failure', 'close-main-failure'):
            start = workflow.index('\n  ' + job + ':\n')
            path.write_text(workflow[:start] + workflow[start:].replace(', check-finalization-geometry', '', 1))
            subprocess.run(['git', '-C', str(repo), 'add', '.github/workflows/ci.yml'], check=True)
            self.commit_geometry(repo)
            with self.subTest(job=job), self.assertRaisesRegex(AssertionError, job):
                geometry_source_preflight(repo, 'HEAD')

    def test_geometry_preflight_runs_before_dependency_setup(self):
        geometry = WORKFLOW.split('\n  check-finalization-geometry:\n', 1)[1].split('\n  check-fast:', 1)[0]
        command = 'python3 -B tests/e2e/history_stock_prepare.test.py --geometry-source-preflight HEAD'
        self.assertLess(geometry.index(command), geometry.index('uses: pnpm/action-setup@'))
        self.assertLess(geometry.index('uses: actions/checkout@'), geometry.index(command))

    def test_offline_controls_run_unconditionally_in_normal_ui_job(self):
        ui = WORKFLOW.split('\n  check-ui:\n', 1)[1].split('\n  check-fast:', 1)[0]
        self.assertIn("    if: ${{ needs.plan_check.outputs.ui == 'true' }}", ui)
        offline = next(value for value in ui.split('      - name: ')[1:]
                       if value.startswith('Offline lifecycle and stock-publication regressions'))
        self.assertNotIn('if:', offline)
        self.assertNotIn('continue-on-error:', offline)
        self.assertNotIn('working-directory:', offline)
        self.assertIn('          node --test ui/scripts/lifecycle-close-drain.selftest.mjs\n', offline)
        for filename in ('history_stock_prepare', 'history_runtime_publication', 'history_controlled_artifact'):
            self.assertIn('          python3 -B tests/e2e/' + filename + '.test.py\n', offline)
        self.assertLess(ui.index('uses: actions/setup-node@'), ui.index(offline))
        self.assertLess(ui.index(offline), ui.index('      - name: ./dev.py check (ui/specs)'))
        self.assertIn('run: ./dev.py check --lanes tsc,ui-lint,vitest,ast-grep,allium,spec-shape,spec-anchors,pkglock', ui)

    def test_existing_harness_commit_binding_covers_the_diagnostic_wrapper(self):
        self.assertIn('ref: ${{ github.event.pull_request.head.sha }}\n          path: harness', LANE)
        self.assertIn("'harness_source': '${{ github.event.pull_request.head.sha }}'", LANE)
        self.assertIn("receipt = json.loads((root.parent/'opening-codegen-receipt.json').read_text())", LANE)
        self.assertIn("(root.parent/'opening-stock-instrumented-receipt.json').write_text(json.dumps(receipt,indent=2))", LANE)
        driver = (ROOT / 'tests/e2e/history_opening_browser.py').read_text()
        self.assertIn('"harness_source": shipping.git(ROOT, "rev-parse", "HEAD")', driver)
        self.assertIn('if any(receipt.get(k) != v for k, v in measured.items()):', driver)
        wrapper = ROOT / 'ui/scripts/history-joint-telemetry.mjs'
        self.assertTrue(wrapper.is_file())
        self.assertIn('helper_sha256: sha256(readFileSync(fileURLToPath(import.meta.url)))', wrapper.read_text())

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
        self.assertIn('include-hidden-files: true', upload)
        for name in ('pinned-app/ui/dist/', 'pinned-app/ui/stock-natural-build.json',
                     'pinned-app/ui/dist-controlled-functional/', 'pinned-app/ui/controlled-functional-build.json',
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
        self.stock = artifact.StockArtifact(self.app / 'ui/dist', 'a' * 64,
                                            (('.gitkeep', hashlib.sha256(b'').hexdigest()),
                                             ('index.html', 'b' * 64)))
        self.receipt = {'app_source': 'f16afb227d9336a863c0b0345960ea9a6d62b7c7',
                        'app_tree': '369305dcd94862d4ee12e7af9fd463d5da14d808',
                        'executable_sha256': self.sha,
                        'ui_files': {name:sha for name,sha in self.stock.ui_files if name != '.gitkeep'},
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
        self.assertEqual(value['stock_ui_files'], dict(self.stock.ui_files))
        self.assertEqual(value['stock_served_ui_files'], self.receipt['ui_files'])
        self.assertIn('.gitkeep', value['stock_ui_files'])
        self.assertNotIn('.gitkeep', value['stock_served_ui_files'])
        self.assertEqual(value['receiving_ui_evidence_class'], 'stock_natural')
        self.assertEqual(value['evidence_class'], 'instrumented_cfg_test_library_not_shipping_runtime')
        self.assertEqual(value['runtime_admission'], 'not_executed')
        self.assertIs(value['full_acceptance'], False)
        self.assertIs(value['overhead_clearance'], False)

    def test_actual_artifact_shape_454_raw_files_and_453_served_files(self):
        files = dict(self.stock.ui_files)
        files.update({f'assets/unit-{number}.js': hashlib.sha256(str(number).encode()).hexdigest()
                      for number in range(452)})
        self.stock = artifact.StockArtifact(self.stock.root, self.stock.manifest_sha256,
                                            tuple(sorted(files.items())))
        self.receipt['ui_files'] = {name:sha for name,sha in files.items() if name != '.gitkeep'}
        (self.out / 'opening-instrumented-receipt.json').write_text(json.dumps(self.receipt))
        value = self.retain()
        owner = self.bind()
        self.assertEqual(len(value['stock_ui_files']), 454)
        self.assertEqual(len(value['stock_served_ui_files']), 453)
        self.assertEqual(value['stock_ui_files'], files)
        self.assertEqual(value['stock_served_ui_files'], self.receipt['ui_files'])
        self.assertEqual(value['stock_manifest_sha256'], self.stock.manifest_sha256)
        self.assertEqual(owner['ui_artifacts']['stock_natural']['ui_files'],
                         {'ui/dist/' + name:sha for name,sha in files.items()})

    def test_stock_without_placeholder_remains_exactly_bound(self):
        self.stock = artifact.StockArtifact(self.stock.root, self.stock.manifest_sha256,
                                            tuple(self.receipt['ui_files'].items()))
        value = self.retain()
        self.bind()
        self.assertEqual(value['stock_ui_files'], value['stock_served_ui_files'])

    def test_retain_rejects_nonempty_nested_or_unrecognized_dotfiles(self):
        original = self.stock
        for name, sha in (('.gitkeep', 'c' * 64), ('assets/.gitkeep', hashlib.sha256(b'').hexdigest()),
                          ('.unexpected', hashlib.sha256(b'').hexdigest()),
                          ('assets/.unexpected', hashlib.sha256(b'').hexdigest())):
            files = dict(original.ui_files)
            files[name] = sha
            self.stock = artifact.StockArtifact(original.root, original.manifest_sha256,
                                                tuple(files.items()))
            with self.subTest(name=name), self.assertRaises(AssertionError):
                self.retain()
            self.assertFalse((self.out / 'opening-stock-receiving-build.json').exists())

    def test_retain_rejects_wrong_served_sha_missing_extra_and_changed_index(self):
        original = self.stock
        for change in ('wrong_served_sha', 'missing', 'extra', 'changed_index'):
            files = dict(original.ui_files)
            served = dict(self.receipt['ui_files'])
            if change == 'wrong_served_sha':
                served['index.html'] = 'f' * 64
            elif change == 'missing':
                served.pop('index.html')
            elif change == 'extra':
                files['assets/extra.js'] = 'f' * 64
            else:
                files['index.html'] = 'f' * 64
            self.stock = artifact.StockArtifact(original.root, original.manifest_sha256,
                                                tuple(files.items()))
            receipt = dict(self.receipt, ui_files=served)
            (self.out / 'opening-instrumented-receipt.json').write_text(json.dumps(receipt))
            with self.subTest(change=change), self.assertRaisesRegex(AssertionError, 'served UI inventory'):
                self.retain()
            self.assertFalse((self.out / 'opening-stock-receiving-build.json').exists())

    def test_binding_rejects_dropped_placeholder_or_unserved_extra_in_raw_inventory(self):
        self.retain()
        path = self.out / 'opening-stock-receiving-build.json'
        original = json.loads(path.read_text())
        for change in ('missing_placeholder', 'extra_raw', 'changed_index'):
            value = json.loads(json.dumps(original))
            if change == 'missing_placeholder':
                value['stock_ui_files'].pop('.gitkeep')
            elif change == 'extra_raw':
                value['stock_ui_files']['assets/extra.js'] = 'f' * 64
            else:
                value['stock_ui_files']['index.html'] = 'f' * 64
                value['stock_served_ui_files']['index.html'] = 'f' * 64
                receipt = dict(self.receipt, ui_files=value['stock_served_ui_files'])
                (self.out / 'opening-instrumented-receipt.json').write_text(json.dumps(receipt))
            path.write_text(json.dumps(value))
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.bind()
            self.assertFalse((self.out / 'opening-stock-instrumented-receipt.json').exists())

    def test_existing_receipt_producer_keeps_its_original_file_filter(self):
        producer = step('Verify and retain instrumented libtest')
        self.assertIn("'ui_files':{str(path.relative_to(root/'ui/dist')):hashlib.sha256(path.read_bytes()).hexdigest()", producer)
        self.assertIn("if path.is_file() and path.name != '.gitkeep'", producer)

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
                 'ui_artifacts': {'stock_natural': {'manifest_sha256': self.stock.manifest_sha256,
                                                   'ui_files': {'ui/dist/' + name:sha for name,sha in self.stock.ui_files}},
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
                                 ('stock_ui_files', {'index.html': 'f' * 64}),
                                 ('stock_served_ui_files', {'index.html': 'f' * 64})):
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
    if sys.argv[1:2] == ['--geometry-source-preflight']:
        assert len(sys.argv) == 3, 'expected one selected Git head'
        selected, blobs, hashes = geometry_source_preflight(ROOT, sys.argv[2])
        assert subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD']).decode().strip() == selected, 'selected head must match checkout'
        print(json.dumps({'selected_head': selected, 'resource_count': len(blobs), 'source_sha256': hashes}, sort_keys=True))
    else:
        unittest.main()
