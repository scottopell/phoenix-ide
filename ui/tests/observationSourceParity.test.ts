import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const root = resolve(import.meta.dirname, '../..');
const manifest = JSON.parse(readFileSync(resolve(root, 'docs/conversation-observation-source-parity.json'), 'utf8')) as {
  base: string;
  product: string;
  diff_args: string[];
  files: Array<{ path: string; base_blob: string | null; product_blob: string | null; actual_blob: string | null; exception?: { reason: string; diff: string } }>;
  approved_backport: { base: string; upstream: string; paths: string[] };
};
const git = (...args: string[]) => execFileSync('git', args, { cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 });
const attribution = JSON.parse(readFileSync(resolve(root, 'docs/conversation-observation-attribution.json'), 'utf8')) as {
  base: string;
  approved_sql_backport: { base: string; product_base: string; upstream: string; paths: string[]; patch_args: string[] };
  diff_args: string[];
  files: Array<{ path: string; base_blob: string | null; actual_blob: string; diff: string; changed_functions: string[] }>;
};
const attributionPaths = [
  'ui/src/pages/ProductConversationPage.tsx',
  'ui/src/pages/ProductConversationPage.test.tsx',
  'crates/phoenix-db/src/product_conversation_read.rs',
  'crates/phoenix-ide/src/api/auth.rs',
  'crates/phoenix-ide/src/api/observation.rs',
  'crates/phoenix-ide/src/api/product_conversations.rs',
  'crates/phoenix-ide/src/observation_startup.rs',
  'crates/phoenix-ide/src/observation_startup/attribution_tests.rs',
  'docs/conversation-observation-attribution.md',
  'docs/conversation-observation-composition.md',
  'docs/conversation-observation-source-parity.json',
  'tests/devpy/test_observation_attribution_source.py',
  'ui/tests/observationSourceParity.test.ts',
];
function tree(ref: string): Map<string, string> {
  return new Map(git('ls-tree', '-r', ref, '--', 'ui').trim().split('\n').filter(Boolean).map(line => {
    const [metadata, path] = line.split('\t');
    return [path!, metadata!.split(' ')[2]!];
  }));
}
function actualBlob(path: string): string | null {
  const absolute = resolve(root, path);
  if (!existsSync(absolute)) return null;
  const bytes = readFileSync(absolute);
  return createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex');
}
const exactDiffArgs = [
  '-c', 'diff.suppressBlankEmpty=false', 'diff',
  '--diff-algorithm=myers', '--no-indent-heuristic', '--unified=3', '--inter-hunk-context=0',
  '--no-ext-diff', '--no-textconv', '--no-color', '--full-index',
  '--src-prefix=a/', '--dst-prefix=b/', '--no-index',
];
function exactDiff(path: string, productBlob: string | null, env = process.env, body = readFileSync(resolve(root, path))): string {
  const scratch = mkdtempSync(resolve(tmpdir(), 'observation-source-diff-'));
  try {
    const product = `${scratch}/product`;
    const actual = `${scratch}/actual`;
    writeFileSync(product, productBlob ? execFileSync('git', ['cat-file', 'blob', productBlob], { cwd: root }) : '');
    writeFileSync(actual, body);
    const diff = spawnSync('git', [...exactDiffArgs, '--', product, actual], { cwd: root, env, encoding: 'utf8' });
    if (diff.status !== 1) throw new Error(`Expected an exception diff for ${path}: ${diff.stderr}`);
    return diff.stdout.replaceAll(product.slice(1), path).replaceAll(actual.slice(1), path);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
const approvedBackport = {
  base: '963307eb834fee0568fb8f991164d842d0ec68c2',
  upstream: 'e34ff8b85148bdb79f8c4164c5a7b067c930421a',
  paths: ['ui/src/pages/ProductConversationPage.tsx', 'ui/src/pages/ProductConversationPage.test.tsx'],
};
function appliedBackport(): Map<string, Buffer> {
  const patch = git(...exactDiffArgs.slice(0, -1), `${approvedBackport.upstream}^`, approvedBackport.upstream, '--', ...approvedBackport.paths);
  const scratch = mkdtempSync(resolve(tmpdir(), 'observation-approved-backport-'));
  try {
    for (const path of approvedBackport.paths) {
      const destination = resolve(scratch, path);
      mkdirSync(dirname(destination), { recursive: true });
      writeFileSync(destination, execFileSync('git', ['show', `${approvedBackport.base}:${path}`], { cwd: root }));
    }
    execFileSync('git', ['apply', '--whitespace=error-all', '-'], { cwd: scratch, input: patch });
    return new Map(approvedBackport.paths.map(path => [path, readFileSync(resolve(scratch, path))]));
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
const sqlPatchArgs = exactDiffArgs.slice(0, -1).map(arg => arg === '--unified=3' ? '--unified=20' : arg);
const approvedSQLBackport = {
  base: 'f9477018529420b462c896d83c115dbc318d8ef9',
  product_base: '250f3d56b13875caa3c927d453ecb70db406803f',
  upstream: 'f16afb227d9336a863c0b0345960ea9a6d62b7c7',
  paths: ['crates/phoenix-db/src/product_conversation_read.rs'],
  patch_args: sqlPatchArgs,
};
function appliedSQLBackport(): Buffer {
  const path = approvedSQLBackport.paths[0]!;
  const patch = git(...sqlPatchArgs, approvedSQLBackport.product_base, approvedSQLBackport.upstream, '--', path);
  const scratch = mkdtempSync(resolve(tmpdir(), 'observation-approved-sql-backport-'));
  try {
    const destination = resolve(scratch, path);
    mkdirSync(dirname(destination), { recursive: true });
    writeFileSync(destination, execFileSync('git', ['show', `${approvedSQLBackport.base}:${path}`], { cwd: root }));
    execFileSync('git', ['apply', '--whitespace=error-all', '-'], { cwd: scratch, input: patch });
    return readFileSync(destination);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
function requireApprovedSQLBytes(body: Buffer, expected: Buffer): void {
  if (!body.equals(expected)) throw new Error('DB differs from exact 250-to-f16 patch applied to f947');
}
function sourcePaths(dir: string): string[] {
  return readdirSync(resolve(root, dir), { withFileTypes: true }).flatMap(entry => {
    const path = `${dir}/${entry.name}`;
    return entry.isDirectory() ? sourcePaths(path) : [path];
  });
}

// The manifest contains exact bytes of each reviewed exception, not permission
// for arbitrary edits to any file named by the allowlist.
describe('observation composition source identity', () => {
  it('pins both authorities and inventories every product/base UI source blob', () => {
    expect(manifest.base).toBe('f3893088b3cb224be9403f600173d5216e3a2f6b');
    expect(manifest.product).toBe('9ee218d4a33ebd4b94ea547d60caa62134edb960');
    expect(manifest.diff_args).toEqual(exactDiffArgs);
    const base = tree(manifest.base);
    const product = tree(manifest.product);
    const paths = new Set([...base.keys(), ...product.keys(), ...sourcePaths('ui/src')]);
    expect(manifest.files.map(file => file.path)).toEqual([...paths].sort());
    for (const file of manifest.files) {
      expect(file.base_blob, file.path).toBe(base.get(file.path) ?? null);
      expect(file.product_blob, file.path).toBe(product.get(file.path) ?? null);
      expect(actualBlob(file.path), file.path).toBe(file.actual_blob);
      if (!file.exception) expect(file.actual_blob, file.path).toBe(file.product_blob);
      else {
        expect(file.actual_blob, file.path).not.toBe(file.product_blob);
        expect(file.exception.reason.length, file.path).toBeGreaterThan(0);
        expect(file.exception.diff, file.path).toBe(exactDiff(file.path, file.product_blob));
      }
    }
  });

  function assertExactPatchesAndRejectBodyMutation(env: NodeJS.ProcessEnv) {
    for (const file of manifest.files.filter(file => file.exception)) {
      expect(exactDiff(file.path, file.product_blob, env), file.path).toBe(file.exception!.diff);
    }
    for (const file of attribution.files) {
      expect(exactDiff(file.path, file.base_blob, env), file.path).toBe(file.diff);
    }
    const file = attribution.files.find(file => file.path === 'crates/phoenix-db/src/product_conversation_read.rs')!;
    const body = Buffer.from(readFileSync(resolve(root, file.path), 'utf8').replace('SELECT kind, ordinary_lifecycle', 'SELECT  kind, ordinary_lifecycle'));
    const changedBlob = createHash('sha1').update(`blob ${body.length}\0`).update(body).digest('hex');
    expect(changedBlob).not.toBe(file.actual_blob);
    expect(exactDiff(file.path, file.base_blob, env, body)).not.toBe(file.diff);
  }

  for (const [key, value] of [['diff.interHunkContext', '1000'], ['diff.suppressBlankEmpty', 'true']]) {
    it(`pins exact patches under ${key}=${value} and rejects body mutation`, () => {
      assertExactPatchesAndRejectBodyMutation({
        ...process.env, GIT_CONFIG_COUNT: '1',
        GIT_CONFIG_KEY_0: key, GIT_CONFIG_VALUE_0: value,
      });
    });
  }

  for (const algorithm of ['histogram', 'myers']) {
    for (const indent of ['true', 'false']) {
      for (const compact of ['true', 'false']) {
        it(`pins exact patches under algorithm=${algorithm}, indent=${indent}, compaction=${compact} and rejects body mutation`, () => {
          const env = {
            ...process.env, GIT_CONFIG_COUNT: '4',
            GIT_CONFIG_KEY_0: 'diff.algorithm', GIT_CONFIG_VALUE_0: algorithm,
            GIT_CONFIG_KEY_1: 'diff.indentHeuristic', GIT_CONFIG_VALUE_1: indent,
            GIT_CONFIG_KEY_2: 'diff.compactionHeuristic', GIT_CONFIG_VALUE_2: compact,
            GIT_CONFIG_KEY_3: 'diff.context', GIT_CONFIG_VALUE_3: '17',
          };
          assertExactPatchesAndRejectBodyMutation(env);
        });
      }
    }
  }

  it('preserves the backend outside exact attribution diffs, generated types and rig support', () => {
    expect(git('diff', '--name-only', manifest.base, '--', '.', ':(exclude)ui/src', ':(exclude)ui/tests/observationSourceParity.test.ts', ':(exclude)docs/conversation-observation-source-parity.json', ':(exclude)docs/conversation-observation-composition.md', ':(exclude)docs/conversation-observation-attribution.json', ...attributionPaths.map(path => `:(exclude)${path}`)).trim()).toBe('');
    for (const file of manifest.files.filter(file => file.path.startsWith('ui/src/generated/') || file.path.startsWith('ui/scripts/'))) {
      expect(file.actual_blob, file.path).toBe(file.base_blob);
    }
  });

  it('bounds attribution and the exact e34-on-963 backport, preserving all other 2702 UI bytes', () => {
    expect(manifest.approved_backport).toEqual(approvedBackport);
    const approved = appliedBackport();
    for (const [path, body] of approved) {
      expect(readFileSync(resolve(root, path)).equals(body), path).toBe(true);
      const baseline = execFileSync('git', ['show', `${approvedBackport.base}:${path}`], { cwd: root });
      for (const mutation of [baseline, Buffer.concat([body, Buffer.from('\n')]), Buffer.from(body.toString().replace('close', 'unapproved_close'))]) {
        expect(mutation.equals(body), path).toBe(false);
      }
    }
    expect(attribution.base).toBe('2702d8389c2fc78ac2e528c876c35a1a7040d692');
    expect(attribution.approved_sql_backport).toEqual(approvedSQLBackport);
    const approvedSQL = appliedSQLBackport();
    const dbPath = approvedSQLBackport.paths[0]!;
    requireApprovedSQLBytes(readFileSync(resolve(root, dbPath)), approvedSQL);
    const dbSource = approvedSQL.toString();
    const oracle = dbSource.split('const ORIGINAL_PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL:')[1]!.split('";')[0]!;
    const page = dbSource.split('const PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL:')[1]!.split('";')[0]!;
    expect(oracle).not.toContain('snapshot_ceiling AS MATERIALIZED (');
    expect(page).toContain('snapshot_ceiling AS MATERIALIZED (');
    for (const selector of ['late_hydration_matches_full_row_oracle_matrix', 'message_page_query_enforces_transcript_first_indexed_join', 'message_page_materializes_bound_snapshot_ceiling_once_with_result_parity']) {
      expect(dbSource.split(`async fn ${selector}(`)).toHaveLength(2);
    }
    for (const [before, after] of [
      ['SELECT kind, ordinary_lifecycle', 'SELECT  kind, ordinary_lifecycle'],
      ['snapshot_ceiling AS MATERIALIZED (', 'snapshot_ceiling AS MATERIALIZED  ('],
      ['.get_conversation(&id)', '.get_conversation_by_slug(&id)'],
      ['snapshot_ceiling AS MATERIALIZED (', 'snapshot_ceiling AS ('],
      ['SELECT sqlite_version()', "SELECT 'invented-version'"],
      ['assert_eq!(ceilings.len(), 3)', 'assert_eq!(ceilings.len(), 4)'],
      ['ceiling fixture continuation', 'unapproved fixture continuation'],
    ]) {
      expect(dbSource).toContain(before);
      expect(() => requireApprovedSQLBytes(Buffer.from(dbSource.replace(before!, after!)), approvedSQL)).toThrow('DB differs');
    }
    const wrongOwner = dbSource.replace('snapshot_ceiling AS MATERIALIZED (', 'snapshot_ceiling AS (').replace('snapshot_ceiling AS (', 'snapshot_ceiling AS MATERIALIZED (');
    expect(() => requireApprovedSQLBytes(Buffer.from(wrongOwner), approvedSQL)).toThrow('DB differs');
    const sqlBaseline = execFileSync('git', ['show', `${approvedSQLBackport.base}:${dbPath}`], { cwd: root });
    for (const mutation of [sqlBaseline, Buffer.concat([approvedSQL, Buffer.from('\n')])]) {
      expect(() => requireApprovedSQLBytes(mutation, approvedSQL)).toThrow('DB differs');
    }
    const sqlPacketPaths = [dbPath, 'docs/conversation-observation-attribution.json', 'docs/conversation-observation-source-parity.json', 'docs/conversation-observation-attribution.md', 'docs/conversation-observation-composition.md', 'tests/devpy/test_observation_attribution_source.py', 'ui/tests/observationSourceParity.test.ts'];
    const sqlChanged = [
      ...git('diff', '--name-only', approvedSQLBackport.base).trim().split('\n').filter(Boolean),
      ...git('ls-files', '--others', '--exclude-standard').trim().split('\n').filter(Boolean),
    ];
    expect(sqlChanged.filter(path => !sqlPacketPaths.includes(path))).toEqual([]);
    expect(attribution.files.find(file => file.path === dbPath)!.changed_functions).toEqual([
      'ReadStage::name', 'ReadCompletion::new', 'ReadCompletion::complete', 'ReadCompletion::emit', 'ReadCompletion::drop', 'complete_read', 'get_ordinary_product_conversation', 'read_ordinary_product_conversation_snapshot', 'get_product_conversation_on',
      'PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL (snapshot_ceiling CTE)', 'tests::message_page_materializes_bound_snapshot_ceiling_once_with_result_parity', 'tests::bound_ceiling_fixture', 'tests::assert_materialized_ceiling_plan',
    ]);
    expect(attribution.diff_args).toEqual(exactDiffArgs);
    expect(attribution.files.map(file => file.path)).toEqual([...attributionPaths].sort());
    for (const file of attribution.files) {
      const entry = git('ls-tree', attribution.base, '--', file.path).trim();
      expect(file.base_blob, file.path).toBe(entry ? entry.split(/\s+/)[2] : null);
      expect(actualBlob(file.path), file.path).toBe(file.actual_blob);
      expect(exactDiff(file.path, file.base_blob), file.path).toBe(file.diff);
    }
    const pinnedUI = tree(attribution.base);
    const currentUI = git('ls-files', '--cached', '--others', '--exclude-standard', '--', 'ui').trim().split('\n').filter(Boolean).sort();
    expect(currentUI).toEqual([...pinnedUI.keys()].sort());
    for (const [path, blob] of pinnedUI) {
      if (path === 'ui/tests/observationSourceParity.test.ts') continue;
      if (approved.has(path)) expect(readFileSync(resolve(root, path)).equals(approved.get(path)!), path).toBe(true);
      else expect(actualBlob(path), path).toBe(blob);
    }
    const changed = new Set([
      ...git('diff', '--name-only', attribution.base).trim().split('\n').filter(Boolean),
      ...git('ls-files', '--others', '--exclude-standard').trim().split('\n').filter(Boolean),
    ]);
    expect([...changed].sort()).toEqual([...attributionPaths, 'docs/conversation-observation-attribution.json'].sort());
  });

  it('permits only enumerated capability adapters and the exact approved product/test backport', () => {
    const adapters = [
      'ui/src/App.markdownNavigation.test.tsx', 'ui/src/App.tsx',
      'ui/src/components/ChainWorkIdentityBlock.tsx',
      'ui/src/contexts/ApplicationCapabilities.tsx', 'ui/src/conversation/ConversationProvider.tsx',
      'ui/src/generated/ApplicationCapabilities.ts', 'ui/src/generated/CapabilityAvailability.ts',
      'ui/src/generated/CoordinatorObservationTarget.ts', 'ui/src/generated/ObservationTargetResponse.ts',
      'ui/src/hooks/conversationViewLifecycle.ts', 'ui/src/hooks/useConversationViewJourney.ts',
      'ui/src/pages/PersistedObservation.test.tsx', 'ui/src/pages/PersistedObservation.tsx',
      'ui/src/pages/ProductConversationPage.tsx',
      'ui/src/pages/ProductConversationPage.test.tsx',
    ];
    expect(manifest.files.filter(file => file.exception && file.path.startsWith('ui/src/')).map(file => file.path)).toEqual(adapters.sort());
    for (const path of adapters.filter(path => !['ui/src/App.tsx', 'ui/src/pages/PersistedObservation.test.tsx', ...approvedBackport.paths].includes(path))) {
      const file = manifest.files.find(file => file.path === path)!;
      expect(file.actual_blob, path).toBe(file.base_blob);
    }
    for (const path of ['ui/src/components/MessageList.tsx', 'ui/src/components/VirtualTranscript.tsx', 'ui/src/components/ConversationNavStack.tsx', 'ui/src/components/ConversationNav.tsx']) {
      const file = manifest.files.find(file => file.path === path)!;
      expect(file.actual_blob, path).toBe(file.product_blob);
      expect(file.exception, path).toBeUndefined();
    }
  });
});
