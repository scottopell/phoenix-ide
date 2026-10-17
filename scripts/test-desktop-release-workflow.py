#!/usr/bin/env python3
import itertools
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

workflow = Path('.github/workflows/release.yml').read_text()
required_fragments = [
    "if: github.ref == 'refs/heads/main'",
    'permissions:\n      contents: read',
    'tag-release:',
    "if: needs.gate.outputs.create_tag == 'true'",
    'echo "create_tag=false" >> "$GITHUB_OUTPUT"',
    'operation:',
    'default: retry-release',
    '- prepare-main',
    '- retry-release',
    'OPERATION: ${{ inputs.operation }}',
    'RETRY_TAG: ${{ inputs.tag }}',
    'Preparing protected desktop artifacts from exact main commit $GITHUB_SHA without a tag or release.',
    'echo "preparation=true" >> "$GITHUB_OUTPUT"',
    'echo "release=false" >> "$GITHUB_OUTPUT"',
    "needs.gate.outputs.preparation == 'true' || (needs.gate.outputs.release == 'true'",
    'Retrying $TAG from its immutable main commit $TAG_COMMIT.',
    'git merge-base --is-ancestor "$TAG_COMMIT" origin/main',
    'VERSION=$(python3 scripts/release_version.py validate-tag "$TAG")',
    'VERSION=$(python3 scripts/release_version.py validate "$VERSION")',
    'channel: ${{ steps.ver.outputs.channel }}',
    '*) CHANNEL=stable ;;',
    '*-rc.*) CHANNEL=rc ;;',
    'echo "channel=$CHANNEL" >> "$GITHUB_OUTPUT"',
    '"${{ needs.gate.outputs.channel }}"',
    'commit: ${{ steps.ver.outputs.commit }}',
    'ref: ${{ needs.gate.outputs.commit }}',
    'ref: ${{ github.sha }}',
    'environment: macos-release-signing',
    'DEVELOPER_ID_P12_BASE64',
    'DEVELOPER_ID_P12_PASSWORD',
    'APPLE_TEAM_ID',
    'APP_STORE_CONNECT_ISSUER_ID',
    'APP_STORE_CONNECT_KEY_ID',
    'APP_STORE_CONNECT_API_KEY',
    'KEYCHAIN_PASSWORD="$(openssl rand -hex 32)"',
    'umask 077',
    'codesign --force --sign "$MACOS_SIGNING_IDENTITY" --options runtime --timestamp',
    'security set-key-partition-list',
    'security delete-keychain',
    'security lock-keychain',
    '[[ ! -e "$RUNNER_TEMP/phoenix-notary-api-key.p8" ]]',
    'rm -f "$RUNNER_TEMP/developer-id.p12" "$RUNNER_TEMP/phoenix-notary-api-key.p8"',
    'macos/Phoenix/scripts/package-desktop-release.sh',
    'needs: [gate, tag-release, build-linux, build-macos]',
    'Phoenix-macos-x86_64-apple-darwin-${{ needs.gate.outputs.tag }}.zip',
    'Phoenix-macos-aarch64-apple-darwin-${{ needs.gate.outputs.tag }}.zip',
    'phoenix_ide-x86_64-unknown-linux-musl-debug',
    'phoenix_ide-aarch64-unknown-linux-musl-debug',
    'sha256sum "${required[@]}" > SHA256SUMS',
    'assets=("${required[@]}" SHA256SUMS)',
    'bash scripts/publish-release-assets.sh',
    'if: needs.gate.outputs.release == \'true\'',
    'PREPARATION-RECEIPT-${{ matrix.target }}.json',
    'PREPARATION-SHA256SUMS-${{ matrix.target }}',
    '"$RUNNER_TEMP/release-assets/PREPARATION-RECEIPT-${{ matrix.target }}.json"',
    'shasum -a 256 -c',
    'desktop-preparation-${{ matrix.target }}-${{ needs.gate.outputs.commit }}',
    'retention-days: 14',
]
for fragment in required_fragments:
    if fragment not in workflow:
        raise SystemExit(f'missing release workflow contract: {fragment}')

for line in workflow.splitlines():
    if line.lstrip().startswith('- uses:'):
        reference = line.split('@', 1)[1].split()[0]
        if len(reference) != 40 or any(char not in '0123456789abcdef' for char in reference):
            raise SystemExit(f'action is not pinned to a full commit: {line.strip()}')

for forbidden in [
    'validate_bundle_version() {',
    'APPLE_ID:',
    'APPLE_APP_SPECIFIC_PASSWORD',
    'MACOS_CERTIFICATE_P12_BASE64',
    'MACOS_CERTIFICATE_PASSWORD',
    'APPLE_NOTARY_API_PRIVATE_KEY_P8_BASE64',
    'MACOS_DEVELOPER_ID_CERT_SHA256',
    'build-macos-desktop:',
    'ref: ${{ needs.gate.outputs.tag }}',
    'Manual dispatch may retry an existing exact tag but cannot create $TAG.',
    '--clobber',
    'com.apple.security.app-sandbox',
    'verify_public_release_if_present',
]:
    if forbidden in workflow:
        raise SystemExit(f'forbidden release workflow contract: {forbidden}')

build_macos_job = workflow.split('\n  build-macos:\n', 1)[1].split('\n  publish:\n', 1)[0]
publish_job = workflow.split('\n  publish:\n', 1)[1]
if not build_macos_job.startswith('    environment: macos-release-signing\n'):
    raise SystemExit('macOS signing must use the protected macos-release-signing environment')
if not publish_job.startswith('    needs: [gate, tag-release, build-linux, build-macos]\n') or '\n    environment: macos-release-signing\n' not in publish_job:
    raise SystemExit('publication must explicitly observe all gate/tag/build results in the protected environment')
publish_condition = re.search(r'^    if: \$\{\{ (.+) \}\}$', publish_job, re.MULTILINE).group(1)
if '!cancelled()' not in publish_condition:
    raise SystemExit('publication needs an explicit status function to avoid implicit skipped-ancestor success gating')

# Execute the actual job condition, not a separately reimplemented predicate.
# Compare all 2,048 result/output/cancellation combinations to the intended gate.
expression = publish_condition.replace('cancelled()', 'cancelled')
expression = expression.replace('&&', ' and ').replace('||', ' or ').replace('!', 'not ')
fields = ['needs.gate.result', 'needs.tag-release.result', 'needs.build-linux.result',
          'needs.build-macos.result', 'needs.gate.outputs.release', 'needs.gate.outputs.create_tag']
for index, field in enumerate(fields):
    expression = expression.replace(field, f'values[{index}]')
assert re.fullmatch(r"[a-z0-9_\[\]() .'=]+", expression), expression
results = ['success', 'failure', 'cancelled', 'skipped']
for gate, tag, linux, macos, release, create_tag, cancelled in itertools.product(
        results, results, results, results, ['true', 'false'], ['true', 'false'], [True, False]):
    values = [gate, tag, linux, macos, release, create_tag]
    actual = eval(expression, {'__builtins__': {}}, {'values': values, 'cancelled': cancelled})
    expected = (not cancelled and gate == 'success' and release == 'true'
                and linux == 'success' and macos == 'success'
                and (tag == 'success' or (tag == 'skipped' and create_tag == 'false')))
    assert actual == expected, values + [cancelled]

workflow_header = workflow.split('\njobs:\n', 1)[0]
if "inputs.operation == 'prepare-main' && 'preparation' || 'release'" not in workflow_header or 'cancel-in-progress: false' not in workflow_header:
    raise SystemExit('preparation must not replace a pending release workflow')
if workflow.count('concurrency:') != 1:
    raise SystemExit('job-local concurrency can break whole-workflow tag/publication serialization')
if 'Wait for earlier release workflows' in workflow or 'actions/workflows/release.yml/runs' in workflow:
    raise SystemExit('release serialization must not claim an application-managed lossless queue')
if 'ref: ${{ needs.gate.outputs.commit }}' not in build_macos_job:
    raise SystemExit('macOS artifacts must be built from the exact gated commit')
if "needs.gate.outputs.release == 'true'" not in publish_condition:
    raise SystemExit('preparation must not reach release publication')
if 'needs.gate.outputs.preparation' in publish_job:
    raise SystemExit('preparation authority must not enter the publication job')
if 'contents: write' in build_macos_job or 'GH_TOKEN:' in build_macos_job or 'GITHUB_TOKEN' in build_macos_job:
    raise SystemExit('preparation-reachable macOS jobs must not have repository mutation authority')
job_matches = list(re.finditer(r'^  ([a-z][a-z0-9-]*):\n', workflow, re.MULTILINE))
job_sections = {
    match.group(1): workflow[match.end():job_matches[index + 1].start() if index + 1 < len(job_matches) else len(workflow)]
    for index, match in enumerate(job_matches)
}
write_jobs = {name for name, body in job_sections.items() if 'contents: write' in body}
if write_jobs != {'tag-release', 'publish'}:
    raise SystemExit(f'unexpected repository write-capable jobs: {sorted(write_jobs)}')
if "if: needs.gate.outputs.create_tag == 'true'" not in job_sections['tag-release']:
    raise SystemExit('tag writer must be release-only')
if "needs.gate.outputs.release == 'true'" not in publish_condition:
    raise SystemExit('publisher must be release-only')
for forbidden in ['prod deploy', '/api/files/reveal', 'gh release', 'gh api --method POST']:
    if forbidden in build_macos_job:
        raise SystemExit(f'preparation-reachable macOS job has a forbidden side effect: {forbidden}')
if '!cancelled()' not in build_macos_job or "needs.gate.result == 'success'" not in build_macos_job:
    raise SystemExit('protected signing must not start after cancellation or failed gate')
if 'ref: ${{ github.sha }}' not in publish_job:
    raise SystemExit('publication retries must use current protected workflow tooling')

secret_names = set(re.findall(r'\$\{\{ secrets\.([A-Z0-9_]+) \}\}', workflow))
expected_secrets = {
    'DEVELOPER_ID_P12_BASE64',
    'DEVELOPER_ID_P12_PASSWORD',
    'APPLE_TEAM_ID',
    'APP_STORE_CONNECT_ISSUER_ID',
    'APP_STORE_CONNECT_KEY_ID',
    'APP_STORE_CONNECT_API_KEY',
    'GITHUB_TOKEN',
}
if secret_names != expected_secrets:
    raise SystemExit(f'release workflow secret allowlist mismatch: {sorted(secret_names)}')

gate_job = workflow.split('\n  gate:\n', 1)[1].split('\n  tag-release:', 1)[0]
tag_job = workflow.split('\n  tag-release:\n', 1)[1].split('\n  build-linux:', 1)[0]
if 'contents: write' in gate_job:
    raise SystemExit('prepare-main gate must not hold repository mutation authority')
if "if: needs.gate.outputs.create_tag == 'true'" not in tag_job or 'contents: write' not in tag_job:
    raise SystemExit('tag mutation authority must be isolated in the release-only tag job')
if 'git tag -a "$TAG"' not in tag_job or 'git push origin "$TAG"' not in tag_job:
    raise SystemExit('release-only tag job must own exact tag creation')
prepare_branch = gate_job.split('prepare-main)', 1)[1].split('retry-release)', 1)[0]
for forbidden in ['git tag ', 'git push origin', 'release=true', 'TAG=$RETRY_TAG']:
    if forbidden in prepare_branch:
        raise SystemExit(f'prepare-main can mutate release state: {forbidden}')
for required in ['preparation=true', 'release=false', 'create_tag=false', 'tag=', 'commit=$GITHUB_SHA']:
    if required not in prepare_branch:
        raise SystemExit(f'prepare-main boundary missing: {required}')

version_helper = Path('scripts/release_version.py')
stale_retry = subprocess.run(
    [sys.executable, str(version_helper), 'validate-new-from-tags', '0.13.1'],
    input='v0.13.2\n',
    capture_output=True,
    text=True,
)
if stale_retry.returncode == 0 or 'must be newer than released version 0.13.2' not in stale_retry.stderr:
    raise SystemExit('a canceled older release must refuse retry after a newer version exists')
still_valid = subprocess.run(
    [sys.executable, str(version_helper), 'validate-new-from-tags', '0.13.3'],
    input='v0.13.2\n',
    check=True,
    capture_output=True,
    text=True,
)
if still_valid.stdout != '0.13.3\n':
    raise SystemExit('a still-valid canceled release must remain explicitly retryable')

version_check = workflow.index('VERSION=$(python3 scripts/release_version.py validate "$VERSION")')
tag_creation = workflow.index('git tag -a "$TAG"', version_check)
if not version_check < tag_creation:
    raise SystemExit('bounded stable/RC version validation must precede tag creation')
retry_parse = workflow.index('VERSION=$(python3 scripts/release_version.py validate-tag "$TAG")')
retry_channel = workflow.index('case "$VERSION" in', retry_parse)
retry_output = workflow.index('echo "channel=$CHANNEL" >> "$GITHUB_OUTPUT"', retry_channel)
version_channel = workflow.index('case "$VERSION" in', version_check)
version_output = workflow.index('echo "channel=$CHANNEL" >> "$GITHUB_OUTPUT"', version_channel)
if not retry_parse < retry_channel < retry_output or not version_check < version_channel < version_output < tag_creation:
    raise SystemExit('release channel must be derived from the validated stable/RC version')

publish_script = Path('scripts/publish-release-assets.sh').read_text()
for fragment in [
    '"$SCRIPT_DIR/release_version.py" validate-tag "$tag"',
    '[[ "$channel" == "$tag_channel" ]]',
    'gh api "repos/$repo/releases/latest" --jq .tag_name',
    'https://uploads.github.com/repos/$repo/releases/$release_id/assets?name=$name',
    '"$PYTHON3" "$SCRIPT_DIR/release_version.py" validate-new-from-tags "$release_version"',
    '-F prerelease="$expected_prerelease"',
    '-f make_latest="$make_latest"',
]:
    if fragment not in publish_script:
        raise SystemExit(f'missing stable/RC publication contract: {fragment}')
if 'gh release create' in publish_script:
    raise SystemExit('release creation must use explicit channel and latest metadata')
if 'notes_args=()' in publish_script or '"${publication_args[@]}"' not in publish_script:
    raise SystemExit('publication arguments must be nonempty under macOS Bash 3.2 nounset')
publisher_tests = Path('scripts/test-publish-release-assets.sh').read_text()
if 'PUBLISH_BASH=${PUBLISH_BASH:-/bin/bash}' not in publisher_tests or '"$PUBLISH_BASH" "$root/scripts/publish-release-assets.sh"' not in publisher_tests:
    raise SystemExit('publisher regressions must default to the actual macOS system Bash')

verify_script = Path('scripts/verify-published-release.sh').read_text()
for fragment in [
    '"$SCRIPT_DIR/release_version.py" validate-tag "$tag"',
    '[[ "$channel" == "$tag_channel" ]]',
    'gh api "repos/$repo/releases/latest" --jq .tag_name',
    '--argjson prerelease "$expected_prerelease"',
]:
    if fragment not in verify_script:
        raise SystemExit(f'missing stable/RC verification contract: {fragment}')

tag_script = Path('scripts/tag-release.sh').read_text()
for fragment in [
    'git -C "$ROOT" ls-remote --refs --tags origin',
    'python3 "$VERSION_HELPER" validate-new-from-tags "$VERSION"',
    'python3 "$VERSION_HELPER" next-from-tags',
]:
    if fragment not in tag_script:
        raise SystemExit(f'missing release-tag version contract: {fragment}')
for forbidden in ['MINOR + 1', '^[0-9]+\\.[0-9]+\\.[0-9]+$']:
    if forbidden in tag_script:
        raise SystemExit(f'tag release script bypasses shared version contract: {forbidden}')

acceptance = workflow[workflow.index('      - name: Check isolated signed-helper startup'):
                      workflow.index('      - name: Retain successful headless acceptance receipt')]
for fragment in [
    'if [[ "${{ needs.gate.outputs.preparation }}" == true ]]; then',
    'require("./scripts/check-signed-helper.cjs")',
    '"${{ needs.gate.outputs.commit }}"',
    '"${{ needs.gate.outputs.version }}"',
    '"${{ matrix.target }}"',
    '"$archive" \\',
    '"$standalone" \\',
    'HEADLESS-ACCEPTANCE-${{ matrix.target }}.json',
]:
    if fragment not in acceptance:
        raise SystemExit(f'missing unchanged-artifact acceptance: {fragment}')
if not (workflow.index('      - name: Remove protected material') <
        workflow.index('      - name: Check isolated signed-helper startup') <
        workflow.index('          name: desktop-preparation-${{ matrix.target }}-${{ needs.gate.outputs.commit }}')):
    raise SystemExit('runtime acceptance must follow signing cleanup and gate successful retention')
for fragment in [
    'if: failure()',
    'path: qa-artifacts/release-qa/${{ github.run_id }}/${{ github.run_attempt }}/${{ matrix.target }}/runtime/acceptance-receipt.json',
]:
    if fragment not in workflow:
        raise SystemExit('failure upload must contain only a sanitized receipt, not private runtime data')

receipt_names = {}
for outcome, step_name in [('success', 'Retain successful headless acceptance receipt'),
                           ('failure', 'Retain failed headless acceptance receipt')]:
    section = build_macos_job.split(f'      - name: {step_name}\n', 1)[1].split('      - ', 1)[0]
    name = re.search(r'^          name: (.+)$', section, re.MULTILINE).group(1)
    if '${{ github.run_id }}' not in name or '${{ github.run_attempt }}' not in name:
        raise SystemExit('receipt names must preserve evidence across runs and rerun attempts')
    if not name.endswith('-' + outcome):
        raise SystemExit('success and failure receipt uploads must not collide within one attempt')
    receipt_names[outcome] = name
seen_names = set()
for target in ['aarch64-apple-darwin', 'x86_64-apple-darwin']:
    for run in ['100', '101']:
        for attempt in ['1', '2']:
            for outcome, template in receipt_names.items():
                name = template
                for key, value in {'matrix.target': target, 'needs.gate.outputs.commit': 'a' * 40,
                                   'github.run_id': run, 'github.run_attempt': attempt}.items():
                    name = name.replace('${{ ' + key + ' }}', value)
                if name in seen_names:
                    raise SystemExit('receipt uploads must be distinct for every arch/run/attempt/outcome')
                seen_names.add(name)

allium = Path('specs/desktop-release/desktop-release.allium').read_text()
for rule, variable in [('CompleteInaccessiblePublicationBecomesVerified', 'release'),
                       ('VerifiedInaccessiblePublicationBecomesPublic', 'publication'),
                       ('ExactPublicRetryIsIdempotent', 'publication')]:
    section = allium.split('rule ' + rule + ' {', 1)[1].split('\n}', 1)[0]
    if f'requires: required_native_pairs_passed({variable})' not in section:
        raise SystemExit(f'{rule} must require both native acceptance passes')
for fragment in [
    'native_acceptance: NativeAcceptanceState',
    'rule NativeAcceptanceRequiresUnchangedVerifiedPair {',
    'requires: protected_signing_material_removed(pair)',
    'requires: native_isolated_artifact_checks_pass(pair)',
    'ensures: sanitized_native_receipt_retained(pair)',
    'rule NativeAcceptanceFailureBlocksPair {',
    'ensures: pair.native_acceptance = failed',
    'invariant PassedNativeAcceptanceHasVerifiedUnchangedPair {',
    'invariant VerifiedPreparationHasNativeAcceptance {',
    'preparation.verified = (preparation_uses_release_security_path(preparation) and required_native_pairs_passed(preparation))',
    'required_native_pairs_passed(ReleasePublication) and',
]:
    if fragment not in allium:
        raise SystemExit(f'missing native acceptance model contract: {fragment}')

for helper, next_helper in [
    ('required_architecture_pairs_validated(publication)', 'public_release_is_exact(publication)'),
    ('preparation_uses_release_security_path(preparation)', 'preparation_has_no_release_authority(preparation)'),
]:
    contract = allium.split('-- ' + helper + ' -> Boolean\n', 1)[1].split('-- ' + next_helper, 1)[0]
    if 'native_acceptance state, read through' not in contract or 'required_native_pairs_passed' not in contract:
        raise SystemExit(f'{helper} must defer native acceptance to its typed authority')
    for duplicate in ['native isolated artifact checks', 'sanitized receipts retained', 'native_acceptance = passed']:
        if duplicate in contract:
            raise SystemExit(f'{helper} must not duplicate native acceptance checks or results')
notes_helper = 'reviewed_notes_attached_atomically_when_supplied(publication)'
notes_contract = allium.split('-- ' + notes_helper + ' -> Boolean\n', 1)[1].split('-- preparation_identity_is_current_main', 1)[0]
for fragment in ['immutable tagged source', 'nonempty notes', 'visibility/channel fields',
                 'no reviewed notes', 'true without', 'existing generated notes remain',
                 'No subsequent public']:
    if fragment not in notes_contract:
        raise SystemExit(f'atomic optional notes contract missing: {fragment}')
public_transition = allium.split('rule VerifiedInaccessiblePublicationBecomesPublic {', 1)[1].split('\n}', 1)[0]
if 'requires: ' + notes_helper not in public_transition:
    raise SystemExit('private-to-public transition must carry supplied notes in its publication request')
public_retry = allium.split('rule ExactPublicRetryIsIdempotent {', 1)[1].split('\n}', 1)[0]
if notes_helper in public_retry:
    raise SystemExit('already-public retries must not require a notes mutation')

if '        if:' in acceptance:
    raise SystemExit('native headless acceptance must gate preparation and release on default success')
release_upload = build_macos_job.split('          name: macos-${{ matrix.target }}', 1)[1].split('      - uses:', 1)[0]
if 'release-assets/*' in release_upload or 'HEADLESS-ACCEPTANCE' in release_upload:
    raise SystemExit('release payload artifact must not include headless receipts')
for fragment in [
    '${{ runner.temp }}/release-assets/${{ matrix.asset }}',
    '${{ runner.temp }}/release-assets/Phoenix-macos-${{ matrix.target }}-${{ needs.gate.outputs.tag }}.zip',
]:
    if fragment not in release_upload:
        raise SystemExit('release upload must contain the exact architecture pair')
if workflow.index('      - name: Retain successful headless acceptance receipt') > workflow.index('          name: macos-${{ matrix.target }}'):
    raise SystemExit('release payload retention must depend on successful receipt retention')
if 'notes_source="${{ needs.gate.outputs.commit }}:docs/releases/${{ needs.gate.outputs.tag }}.md"' not in publish_job:
    raise SystemExit('reviewed notes must come from immutable gated source, not current retry tooling')
if 'git show "$notes_source" > "$RELEASE_NOTES_FILE"' not in publish_job or 'fetch-depth: 0' not in publish_job:
    raise SystemExit('publisher must retain immutable tagged notes for historical retries')
notes = Path('docs/releases/v0.13.0-rc.2.md').read_text()
for fragment in ['macOS GUI/clean-host desktop pass', 'physical iPhone Safari', 'Known acceptance gaps', 'not new RC2-byte acceptance']:
    if fragment not in notes:
        raise SystemExit(f'missing RC2 disclosure: {fragment}')

qa_root = '$GITHUB_WORKSPACE/qa-artifacts/release-qa/${{ github.run_id }}/${{ github.run_attempt }}/${{ matrix.target }}'
if f'qa="{qa_root}"' not in acceptance or '/target/signed-helper-acceptance/' in acceptance:
    raise SystemExit('runtime QA must be run/attempt-qualified outside the Cargo target cache')
if any(part.startswith('.') for part in qa_root.split('/')[1:]):
    raise SystemExit('receipt parent directories must not depend on hidden-file uploader behavior')
if 'rm -' in acceptance:
    raise SystemExit('stale runtime directories must not be deleted or reused to satisfy freshness')
wrapper = acceptance.split("          node --eval '\n", 1)[1].split("          ' \\\n", 1)[0]
for fragment in ['const { checkSignedHelper } = require("./scripts/check-signed-helper.cjs")',
                 'checkSignedHelper({ archive, standalone, sha, version, target, output })',
                 'preflightCodes.has(error.message)', 'acceptance-failed-see-sanitized-receipt',
                 'process.exitCode = 1']:
    if fragment not in wrapper:
        raise SystemExit(f'workflow must retain the tagged checker and safe failure behavior: {fragment}')
if 'error.stack' in wrapper or 'JSON.stringify(error)' in wrapper:
    raise SystemExit('workflow must not expose private raw checker diagnostics')

# Execute the real workflow shell with a node double: prove each mode selects its
# own bytes and a failed native check cannot retain a successful payload receipt.
acceptance_shell = acceptance.split('        run: |\n', 1)[1]
acceptance_shell = '\n'.join(line[10:] for line in acceptance_shell.splitlines())
with tempfile.TemporaryDirectory() as temporary:
    base = Path(temporary)
    bin_dir = base / 'bin'
    bin_dir.mkdir()
    node = bin_dir / 'node'
    node.write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
Path(os.environ['CALLS']).write_text(json.dumps(sys.argv[1:]))
if os.environ.get('FAIL_CHECK') == '1':
    raise SystemExit(1)
runtime = Path(sys.argv[-1])
runtime.mkdir(parents=True, exist_ok=True)
(runtime / 'acceptance-receipt.json').write_text('{}')
''')
    node.chmod(0o755)
    # Invoke the actual inline wrapper with the unchanged checker on a rejected
    # preflight. It must fail closed and expose only an allowlisted static code.
    result = subprocess.run(['node', '--eval', wrapper, str(base / 'private-archive'),
                             str(base / 'private-standalone'), 'short', '0.13.0-rc.2',
                             'aarch64-apple-darwin', str(base / 'private-output')],
                            capture_output=True, text=True)
    assert result.returncode == 1
    assert result.stderr.strip() in {
        'Signed-helper acceptance failed: requires-macos-host; private details withheld.',
        'Signed-helper acceptance failed: expected-full-sha-required; private details withheld.',
    }, result.stderr
    assert str(base) not in result.stderr and not (base / 'private-output').exists()
    private_error = 'private diagnostic path=' + str(base) + ' cookie=DO-NOT-LOG'
    rejection_double = ('require.cache[require.resolve("./scripts/check-signed-helper.cjs")] = '
                        '{exports: {checkSignedHelper: () => Promise.reject(new Error('
                        + json.dumps(private_error) + '))}};\n')
    result = subprocess.run(['node', '--eval', rejection_double + wrapper,
                             'archive', 'standalone', 'a' * 40, '0.13.0-rc.2',
                             'aarch64-apple-darwin', str(base / 'private-output')],
                            capture_output=True, text=True)
    assert result.returncode == 1
    assert result.stderr.strip() == 'Signed-helper acceptance failed: acceptance-failed-see-sanitized-receipt; private details withheld.'
    assert private_error not in result.stderr and not result.stdout
    for target in ['aarch64-apple-darwin', 'x86_64-apple-darwin']:
        for preparation in ['true', 'false']:
            for fail in ['0', '1']:
                case = base / f'{target}-{preparation}-{fail}'
                assets_dir = case / 'release-assets'
                assets_dir.mkdir(parents=True)
                old_runtime = case / f'target/signed-helper-acceptance/{target}/runtime'
                old_runtime.mkdir(parents=True)
                marker = old_runtime / 'prior-proof'
                marker.write_text('preserve old cache evidence')
                prior_attempt = case / f'qa-artifacts/release-qa/100/1/{target}/runtime'
                prior_attempt.mkdir(parents=True)
                (prior_attempt / 'prior-proof').write_text('preserve prior attempt evidence')
                substitutions = {
                    'needs.gate.outputs.commit': 'a' * 40,
                    'needs.gate.outputs.version': '0.13.0-rc.2',
                    'needs.gate.outputs.tag': 'v0.13.0-rc.2',
                    'needs.gate.outputs.preparation': preparation,
                    'matrix.target': target,
                    'matrix.asset': f'phoenix_ide-{target}',
                    'github.run_id': '100',
                    'github.run_attempt': '2',
                }
                shell = acceptance_shell
                for key, value in substitutions.items():
                    shell = shell.replace('${{ ' + key + ' }}', value)
                env = dict(os.environ, PATH=f'{bin_dir}{os.pathsep}{os.environ["PATH"]}',
                           GITHUB_WORKSPACE=str(case), RUNNER_TEMP=str(case),
                           CALLS=str(case / 'calls.json'), FAIL_CHECK=fail)
                result = subprocess.run(['bash', '-euo', 'pipefail', '-c', shell], env=env, capture_output=True, text=True)
                assert result.returncode == (1 if fail == '1' else 0), result.stderr
                calls = json.loads((case / 'calls.json').read_text())
                suffix = '-prepared-' + 'a' * 12 if preparation == 'true' else ''
                archive_suffix = suffix if preparation == 'true' else '-v0.13.0-rc.2'
                assert calls[0] == '--eval'
                assert calls[2] == str(assets_dir / f'Phoenix-macos-{target}{archive_suffix}.zip')
                assert calls[3] == str(assets_dir / f'phoenix_ide-{target}{suffix}')
                assert calls[4:7] == ['a' * 40, '0.13.0-rc.2', target]
                runtime = case / f'qa-artifacts/release-qa/100/2/{target}/runtime'
                assert calls[7] == str(runtime)
                assert (runtime / 'acceptance-receipt.json').exists() == (fail == '0')
                assert marker.read_text() == 'preserve old cache evidence'
                assert (prior_attempt / 'prior-proof').read_text() == 'preserve prior attempt evidence'
                receipt = assets_dir / f'HEADLESS-ACCEPTANCE-{target}.json'
                assert receipt.exists() == (preparation == 'true' and fail == '0')

package_script = Path('macos/Phoenix/scripts/package-desktop-release.sh').read_text()
for fragment in [
    '"operation": "prepare-main"',
    '"notarization_submission_id": submission_id',
    '"notarization": status.lower()',
    '"gatekeeper": "accepted"',
]:
    if fragment not in package_script:
        raise SystemExit(f'missing preparation receipt contract: {fragment}')
for fragment in [
    'TeamIdentifier=$APPLE_TEAM_ID',
    '--output-format json',
    '"$notary_status" != Accepted',
    'notarytool log',
    'stapler staple',
    'stapler validate',
    '--verify --deep --strict --verbose=2',
    '--assess --type execute --verbose=2',
    '--sequesterRsrc --keepParent',
    '"$version_helper" validate-tag "$tag"',
    '"$version_helper" apple-marketing "$release_version"',
    '"$version_helper" apple-build "$release_version"',
]:
    if fragment not in package_script:
        raise SystemExit(f'missing package verification contract: {fragment}')
for forbidden in ['com.apple.security.app-sandbox', '--clobber', 'def release_build_number']:
    if forbidden in package_script:
        raise SystemExit(f'forbidden package behavior: {forbidden}')

macos_workflow = Path('.github/workflows/macos-app.yml').read_text()
for line in macos_workflow.splitlines():
    if line.lstrip().startswith('- uses:'):
        reference = line.split('@', 1)[1].split()[0]
        if len(reference) != 40 or any(char not in '0123456789abcdef' for char in reference):
            raise SystemExit(f'macOS action is not pinned to a full commit: {line.strip()}')
for fragment in [
    'macos/Phoenix/scripts/test-package-desktop-release.sh',
    'scripts/test-publish-release-assets.sh',
    'scripts/test-verify-published-release.sh',
    'scripts/test-desktop-release-workflow.py',
    'node --test scripts/test-check-signed-helper.cjs',
    '"scripts/check-signed-helper.cjs"',
    '"scripts/test-check-signed-helper.cjs"',
]:
    if fragment not in macos_workflow:
        raise SystemExit(f'missing macOS workflow regression check: {fragment}')

for identity_path in [
    'crates/phoenix-ide/build.rs',
    'crates/phoenix-ide/Cargo.toml',
    'scripts/verify-published-release.sh',
    'scripts/test-verify-published-release.sh',
    'scripts/sign-notarize-browser-cli.sh',
]:
    if macos_workflow.count(f'"{identity_path}"') != 2:
        raise SystemExit(f'macOS workflow must trigger on {identity_path} for pull requests and pushes')

if 'PHOENIX_EXPECTED_BUILD_IDENTITY="$expected_build_identity"' not in package_script:
    raise SystemExit('desktop packaging must pass the complete helper identity into app assembly')

for forbidden in ['rev-parse --short', r'^[0-9a-f]{12}$', '${embedded_commit}^{commit}']:
    if forbidden in package_script:
        raise SystemExit(f'desktop packaging must not accept or resolve abbreviated Git identity: {forbidden}')
if '[[ "$embedded_commit" == "$expected_commit" ]]' not in package_script:
    raise SystemExit('desktop packaging must compare the complete embedded and expected commits')

build_script = Path('crates/phoenix-ide/build.rs').read_text()
if 'git(&["rev-parse", "HEAD"])' not in build_script or '--short' in build_script:
    raise SystemExit('Rust build identity producer must derive the full Git commit')

sidecar_script = Path('macos/Phoenix/scripts/package-sidecar.sh').read_text()
if 'rev-parse HEAD' not in sidecar_script or 'rev-parse --short' in sidecar_script:
    raise SystemExit('generic sidecar packaging must derive the full Git commit')
if '[0-9a-f]{40}(-dirty)?' not in sidecar_script:
    raise SystemExit('generic sidecar validation must require a full Git commit')

if '"$version" "$full_commit"' not in macos_workflow or 'short_commit' in macos_workflow:
    raise SystemExit('real unsigned macOS workflow fixture must embed the full Git commit')


# Source capability is evaluated against the immutable release commit, so a
# retry of a pre-CLI tag keeps its original eight-payload asset contract.
capability = workflow.split('      - name: Resolve browser CLI from immutable source\n', 1)[1].split('\n  tag-release:', 1)[0]
capability_shell = capability.split('        run: |\n', 1)[1]
capability_shell = '\n'.join(line[10:] for line in capability_shell.splitlines())
with tempfile.TemporaryDirectory() as temporary:
    repo = Path(temporary)
    subprocess.run(['git', 'init', '-q', str(repo)], check=True)
    git_env = dict(os.environ, GIT_AUTHOR_NAME='Fixture', GIT_AUTHOR_EMAIL='fixture@example.com',
                   GIT_COMMITTER_NAME='Fixture', GIT_COMMITTER_EMAIL='fixture@example.com')
    (repo / 'README').write_text('historical source')
    subprocess.run(['git', 'add', '.'], cwd=repo, check=True)
    subprocess.run(['git', 'commit', '-qm', 'historical'], cwd=repo, env=git_env, check=True)
    old = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    entrypoint = repo / 'crates/phoenix-tools/src/bin/phoenix-browser.rs'
    entrypoint.parent.mkdir(parents=True)
    entrypoint.write_text('fn main() {}')
    subprocess.run(['git', 'add', '.'], cwd=repo, check=True)
    subprocess.run(['git', 'commit', '-qm', 'browser entrypoint'], cwd=repo, env=git_env, check=True)
    new = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    for commit, release, expected in [(old, 'true', 'false'), (new, 'true', 'true'),
                                       (new, 'false', 'false')]:
        output = repo / 'outputs'
        output.unlink(missing_ok=True)
        subprocess.run(['bash', '-euo', 'pipefail', '-c', capability_shell], cwd=repo,
                       env=dict(os.environ, SOURCE_COMMIT=commit, RELEASE=release,
                                GITHUB_OUTPUT=str(output)), check=True)
        assert output.read_text() == f'browser_cli={expected}\n'

linux_job = job_sections['build-linux']
for section, step in [(linux_job, 'Build and smoke native Linux browser CLI'),
                      (build_macos_job, 'Build native macOS browser CLI')]:
    browser_step = section.split('      - name: ' + step + '\n', 1)[1].split('      - ', 1)[0]
    assert "needs.gate.outputs.browser_cli == 'true'" in browser_step
    assert 'cargo build --locked -p phoenix-tools --bin phoenix-browser --profile release-min --target' in browser_step
    assert '--release ' not in browser_step
assert "if: needs.gate.outputs.release == 'true' && needs.gate.outputs.browser_cli == 'true'" in build_macos_job
assert 'if [[ "${{ needs.gate.outputs.release }}" == true && "${{ needs.gate.outputs.browser_cli }}" == true ]]; then\n            bash scripts/sign-notarize-browser-cli.sh' in build_macos_job
assert 'rm -rf "$RUNNER_TEMP"/phoenix-browser-notary.* || cleanup_failed=1' in build_macos_job
assert '${{ runner.temp }}/release-assets/phoenix-browser-${{ matrix.target }}' in release_upload
assert 'phoenix-browser-${{ matrix.target }}' in linux_job
assert 'phoenix-browser-' not in build_macos_job.split('          name: desktop-preparation-', 1)[1]
required_shell = publish_job.split('          required=(\n', 1)[1].split('          for asset in', 1)[0]
assert 'if [[ "${{ needs.gate.outputs.browser_cli }}" == true ]]; then' in required_shell
for target in ['x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl',
               'x86_64-apple-darwin', 'aarch64-apple-darwin']:
    assert f'phoenix-browser-{target}\n' in required_shell
assert required_shell.count('phoenix-browser-') == 4
for capability, expected_count in [('false', 8), ('true', 12)]:
    shell = 'required=(\n' + required_shell
    shell = shell.replace('${{ needs.gate.outputs.browser_cli }}', capability)
    shell = shell.replace('${{ needs.gate.outputs.tag }}', 'v0.13.0-rc.3')
    shell += '\nprintf "%s\\n" "${required[@]}"\n'
    payloads = subprocess.check_output(['bash', '-euo', 'pipefail', '-c', shell], text=True).splitlines()
    assert len(payloads) == len(set(payloads)) == expected_count
    assert sum(name.startswith('phoenix-browser-') for name in payloads) == expected_count - 8
    assert not any(name.startswith('phoenix-browser-') and name.endswith('-debug') for name in payloads)

# Exercise the real signing/notarization helper with external tools replaced.
# No Apple credentials, network requests, Chrome, or Phoenix server are used.
with tempfile.TemporaryDirectory() as temporary:
    base = Path(temporary)
    tool = base / 'tool'
    tool.write_text(r"""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
with Path(os.environ['CALLS']).open('a') as log:
    log.write(json.dumps([name] + sys.argv[1:]) + '\n')
if name == 'lipo':
    print(os.environ['ARCH'])
elif name == 'codesign' and '--verify' in sys.argv and os.environ.get('VERIFY_FAIL') == 'yes':
    raise SystemExit(1)
elif name == 'codesign' and '--display' in sys.argv:
    print('Authority=' + os.environ.get('AUTHORITY', 'Developer ID Application: Fixture'))
    print('TeamIdentifier=' + os.environ.get('TEAM', 'ABCDEFGHIJ'))
    print('CodeDirectory v=20500 flags=' + os.environ.get('FLAGS', '0x10000(runtime)'))
    if os.environ.get('TIMESTAMP', 'yes') == 'yes':
        print('Timestamp=Fixture timestamp')
elif name == 'ditto':
    Path(sys.argv[-1]).write_bytes(b'notary zip')
elif name == 'xcrun':
    if os.environ.get('SUBMIT_FAIL') == 'yes':
        raise SystemExit(1)
    print(json.dumps({'status': os.environ.get('NOTARY_STATUS', 'Accepted')}))
elif name == 'browser':
    if os.environ.get('SMOKE_FAIL') == 'yes':
        raise SystemExit(1)
""")
    tool.chmod(0o755)
    for name in ['lipo', 'codesign', 'ditto', 'xcrun', 'browser']:
        (base / name).symlink_to(tool)
    key = base / 'key.p8'
    key.write_text('fixture')
    common = dict(os.environ, MACOS_SIGNING_IDENTITY='fixture identity', APPLE_TEAM_ID='ABCDEFGHIJ',
                  APP_STORE_CONNECT_API_KEY_PATH=str(key), APP_STORE_CONNECT_KEY_ID='KLMNOPQRST',
                  APP_STORE_CONNECT_ISSUER_ID='fixture issuer', RUNNER_TEMP=str(base),
                  CODESIGN=str(base / 'codesign'), LIPO=str(base / 'lipo'),
                  DITTO=str(base / 'ditto'), XCRUN=str(base / 'xcrun'),
                  PYTHON3=sys.executable, CALLS=str(base / 'calls'))
    cases = [({}, True), ({'ARCH': 'x86_64', 'TARGET': 'x86_64-apple-darwin'}, True),
             ({'ARCH': 'x86_64'}, False), ({'TEAM': 'ABCDEFGHIJextra'}, False),
             ({'AUTHORITY': 'ad hoc'}, False), ({'FLAGS': '0x0'}, False),
             ({'TIMESTAMP': 'no'}, False), ({'NOTARY_STATUS': 'Invalid'}, False),
             ({'VERIFY_FAIL': 'yes'}, False), ({'SUBMIT_FAIL': 'yes'}, False),
             ({'SMOKE_FAIL': 'yes'}, False)]
    for overrides, success in cases:
        calls = base / 'calls'
        calls.unlink(missing_ok=True)
        env = dict(common, ARCH='arm64')
        env.update(overrides)
        result = subprocess.run(['bash', 'scripts/sign-notarize-browser-cli.sh', str(base / 'browser'),
                                 overrides.get('TARGET', 'aarch64-apple-darwin')],
                                env=env, capture_output=True, text=True)
        assert (result.returncode == 0) == success, (overrides, result.stderr)
        commands = [json.loads(line) for line in calls.read_text().splitlines()]
        assert not list(base.glob('phoenix-browser-notary.*')), overrides
        if success:
            sign = next(command for command in commands if command[:2] == ['codesign', '--force'])
            assert sign[1:-1] == ['--force', '--sign', 'fixture identity', '--options', 'runtime', '--timestamp']
            submit = next(command for command in commands if command[0] == 'xcrun')
            assert submit[1:3] == ['notarytool', 'submit']
            assert submit[-3:] == ['--wait', '--output-format', 'json']
            smoke = [command[1:] for command in commands if command[0] == 'browser']
            assert smoke == [['--help'], ['--json', 'schema', 'snapshot']]
        elif 'SMOKE_FAIL' not in overrides:
            assert not any(command[0] == 'browser' for command in commands)

print('desktop release workflow regression checks passed')
