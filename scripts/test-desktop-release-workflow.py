#!/usr/bin/env python3
from pathlib import Path

workflow = Path('.github/workflows/release.yml').read_text()
required_fragments = [
    'build-macos-desktop:',
    'needs: [gate, build-macos]',
    'name: macos-${{ matrix.target }}',
    'MACOS_CERTIFICATE_P12_BASE64',
    'MACOS_SIGNING_IDENTITY',
    'APPLE_APP_SPECIFIC_PASSWORD',
    'package-desktop-release.sh',
    'needs: [gate, build-linux, build-macos, build-macos-desktop]',
    'Phoenix-macos-x86_64-apple-darwin-${{ needs.gate.outputs.tag }}.zip',
    'Phoenix-macos-aarch64-apple-darwin-${{ needs.gate.outputs.tag }}.zip',
    'sha256sum phoenix_ide-* Phoenix-macos-* > SHA256SUMS',
    'test "$(wc -l < SHA256SUMS)" -eq 8',
    'Tag $TAG already points at this exact commit — retrying its release.',
    'gh release upload "${{ needs.gate.outputs.tag }}"',
    '--clobber',
]
missing = [fragment for fragment in required_fragments if fragment not in workflow]
if missing:
    raise SystemExit('desktop release workflow missing:\n' + '\n'.join(missing))

# Existing standalone server assets remain named exactly as before.
for asset in [
    'phoenix_ide-x86_64-unknown-linux-musl',
    'phoenix_ide-aarch64-unknown-linux-musl',
    'phoenix_ide-x86_64-apple-darwin',
    'phoenix_ide-aarch64-apple-darwin',
]:
    if asset not in workflow:
        raise SystemExit(f'existing server asset disappeared: {asset}')

print('desktop release workflow contract checks passed')
