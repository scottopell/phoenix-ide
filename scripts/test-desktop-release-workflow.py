#!/usr/bin/env python3
import re
from pathlib import Path

workflow = Path('.github/workflows/release.yml').read_text()
required_fragments = [
    "if: github.ref == 'refs/heads/main'",
    'permissions:\n      contents: write',
    'RETRY_TAG: ${{ inputs.tag }}',
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
    'needs: [gate, build-linux, build-macos]',
    'Phoenix-macos-x86_64-apple-darwin-${{ needs.gate.outputs.tag }}.zip',
    'Phoenix-macos-aarch64-apple-darwin-${{ needs.gate.outputs.tag }}.zip',
    'phoenix_ide-x86_64-unknown-linux-musl-debug',
    'phoenix_ide-aarch64-unknown-linux-musl-debug',
    'sha256sum "${required[@]}" > SHA256SUMS',
    'assets=("${required[@]}" SHA256SUMS)',
    'bash scripts/publish-release-assets.sh',
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
if not publish_job.startswith('    needs: [gate, build-linux, build-macos]\n    environment: macos-release-signing\n'):
    raise SystemExit('release publication must use the protected macos-release-signing environment')
if 'concurrency:\n      group: publish-release-assets\n      cancel-in-progress: false' not in publish_job:
    raise SystemExit('release publication must be serialized across tags')
if 'publish-release-assets-${{ needs.gate.outputs.tag }}' in publish_job:
    raise SystemExit('tag-specific publication concurrency permits stable latest races')
if 'concurrency:\n      group: release-tag-gate\n      cancel-in-progress: false' not in workflow.split('\n  build-linux:', 1)[0]:
    raise SystemExit('release tag validation and creation must be serialized')
if 'ref: ${{ needs.gate.outputs.commit }}' not in build_macos_job:
    raise SystemExit('macOS artifacts must be built from the immutable tagged commit')
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

package_script = Path('macos/Phoenix/scripts/package-desktop-release.sh').read_text()
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
]:
    if fragment not in macos_workflow:
        raise SystemExit(f'missing macOS workflow regression check: {fragment}')

for identity_path in [
    'crates/phoenix-ide/build.rs',
    'crates/phoenix-ide/Cargo.toml',
    'scripts/verify-published-release.sh',
    'scripts/test-verify-published-release.sh',
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

print('desktop release workflow regression checks passed')
