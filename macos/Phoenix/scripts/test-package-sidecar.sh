#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
script="$script_dir/package-sidecar.sh"
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

appdir="$tmpdir/Build/Phoenix.app"
helpers="$appdir/Contents/Helpers"
mkdir -p "$helpers"

dummy_sidecar="$tmpdir/phoenix_ide"
cat > "$dummy_sidecar" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$dummy_sidecar"

fakebin="$tmpdir/fakebin"
mkdir -p "$fakebin"
cat > "$fakebin/lipo" <<'EOF'
#!/bin/sh
if [ "$1" = "-archs" ]; then
  printf '%s\n' "${FAKE_LIPO_ARCHS:-arm64 x86_64}"
else
  exit 2
fi
EOF
cat > "$fakebin/codesign" <<'EOF'
#!/bin/sh
if [ "$1" = "--verify" ] && [ "$2" = "--strict" ]; then
  if [ "${FAKE_CODESIGN_REQUIREMENT_RESULT:-unset}" != "unset" ] && [ "$3" = "--requirement" ]; then
    [ "$FAKE_CODESIGN_REQUIREMENT_RESULT" = "ok" ] && exit 0
    exit 1
  fi
  [ "${FAKE_CODESIGN_VERIFY_RESULT:-ok}" = "ok" ] && exit 0
  exit 1
fi
if [ "$1" = "-dv" ]; then
  case "${FAKE_CODESIGN_AUTHORITY:-none}" in
    none)
      exit 0
      ;;
    adhoc)
      printf 'Executable=fake-helper\n'
      exit 0
      ;;
    identity:*)
      printf 'Authority=%s\n' "${FAKE_CODESIGN_AUTHORITY#identity:}"
      exit 0
      ;;
  esac
fi
exit 0
EOF
chmod +x "$fakebin/lipo" "$fakebin/codesign"

run_script() {
  PATH="$fakebin:/usr/bin:/bin" \
  TARGET_BUILD_DIR="$tmpdir/Build" \
  CONTENTS_FOLDER_PATH="Phoenix.app/Contents" \
  ARCHS="${ARCHS_OVERRIDE:-arm64 x86_64}" \
  CONFIGURATION="${CONFIGURATION_OVERRIDE:-Debug}" \
  PHOENIX_SIDECAR_PATH="${PHOENIX_SIDECAR_PATH:-}" \
  CODE_SIGNING_ALLOWED="${CODE_SIGNING_ALLOWED_OVERRIDE:-NO}" \
  CODE_SIGN_STYLE="${CODE_SIGN_STYLE_OVERRIDE:-Manual}" \
  EXPANDED_CODE_SIGN_IDENTITY="${EXPANDED_CODE_SIGN_IDENTITY_OVERRIDE:-}" \
  /bin/sh "$script"
}

# stale helper removed when sidecar omitted in Debug
printf stale > "$helpers/phoenix_ide"
unset PHOENIX_SIDECAR_PATH || true
CONFIGURATION_OVERRIDE=Debug run_script >/dev/null
[ ! -e "$helpers/phoenix_ide" ]

# copied helper remains and architecture/signing validation inspects packaged file
PHOENIX_SIDECAR_PATH="$dummy_sidecar"
FAKE_LIPO_ARCHS='arm64 x86_64'
CODE_SIGNING_ALLOWED_OVERRIDE=NO
run_script >/dev/null
[ -x "$helpers/phoenix_ide" ]

# source inspection regression: keep stale-helper cleanup and packaged validation in the script
/usr/bin/grep '/bin/rm -f "\$destination"' "$script" >/dev/null
/usr/bin/grep 'validate_packaged_sidecar "\$destination" "\$ARCHS" "\$expected_signing"' "$script" >/dev/null
/usr/bin/grep 'packaged sidecar is not signed with expected identity' "$script" >/dev/null

# designated requirement / identity-name verification should be present for explicit identities
/usr/bin/grep 'codesign --verify --strict --requirement' "$script" >/dev/null
if ! FAKE_CODESIGN_REQUIREMENT_RESULT=ok CODE_SIGNING_ALLOWED_OVERRIDE=YES EXPANDED_CODE_SIGN_IDENTITY_OVERRIDE='Developer ID Application: Phoenix' PHOENIX_SIDECAR_PATH="$dummy_sidecar" \
  run_script >/dev/null 2>"$tmpdir/identity.err"; then
  echo 'error: expected explicit identity requirement verification to pass' >&2
  exit 1
fi

# strict codesign verification runs before metadata checks when signing is expected
if FAKE_CODESIGN_VERIFY_RESULT=fail CODE_SIGNING_ALLOWED_OVERRIDE=YES CODE_SIGN_STYLE_OVERRIDE=Automatic PHOENIX_SIDECAR_PATH="$dummy_sidecar" \
  run_script >/dev/null 2>"$tmpdir/strict.err"; then
  echo 'error: expected strict codesign verification to fail' >&2
  exit 1
fi
/usr/bin/grep 'packaged sidecar failed codesign --verify --strict' "$tmpdir/strict.err" >/dev/null

# source inspection regression: keep stale-helper cleanup and packaged validation in the script
/usr/bin/grep '/bin/rm -f "\$destination"' "$script" >/dev/null
/usr/bin/grep 'validate_packaged_sidecar "\$destination" "\$ARCHS" "\$expected_signing"' "$script" >/dev/null
/usr/bin/grep 'codesign --verify --strict' "$script" >/dev/null
/usr/bin/grep 'packaged sidecar is not signed with expected identity' "$script" >/dev/null

echo 'package-sidecar.sh regression checks passed'
