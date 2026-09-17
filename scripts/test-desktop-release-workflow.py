#!/usr/bin/env python3
from pathlib import Path

workflow = Path('.github/workflows/release.yml').read_text()
required_fragments = [
    "if: github.ref == 'refs/heads/main'",
    'permissions:\n      contents: write',
    'RETRY_TAG: ${{ inputs.tag }}',
    'Retrying $TAG from its immutable main commit $TAG_COMMIT.',
    'git merge-base --is-ancestor "$TAG_COMMIT" origin/main',
    'validate_bundle_version "$VERSION"',
    'bash scripts/verify-published-release.sh',
    'echo "release=false" >> "$GITHUB_OUTPUT"',
    'commit: ${{ steps.ver.outputs.commit }}',
    'ref: ${{ needs.gate.outputs.commit }}',
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
]:
    if forbidden in workflow:
        raise SystemExit(f'forbidden release workflow contract: {forbidden}')

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
]:
    if fragment not in package_script:
        raise SystemExit(f'missing package verification contract: {fragment}')
for forbidden in ['com.apple.security.app-sandbox', '--clobber']:
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

print('desktop release workflow regression checks passed')
