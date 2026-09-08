#!/bin/sh
# shellcheck shell=sh

set -eu

if [ "${SKIP_PACKAGE_SIDECAR_MAIN:-0}" = "1" ]; then
  return 0 2>/dev/null || exit 0
fi

helpers="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Helpers"
destination="$helpers/phoenix_ide"

expected_build_identity() {
  if [ -n "${PHOENIX_EXPECTED_BUILD_IDENTITY:-}" ]; then
    printf '%s\n' "$PHOENIX_EXPECTED_BUILD_IDENTITY"
    return 0
  fi
  repository_root="${PHOENIX_REPOSITORY_ROOT:-}"
  if [ -z "$repository_root" ] && [ -n "${SRCROOT:-}" ]; then
    repository_root="$(CDPATH= cd -- "$SRCROOT/../.." && pwd)"
  fi
  [ -n "$repository_root" ] || return 1
  version="$(/usr/bin/grep -m1 '^version' "$repository_root/crates/phoenix-ide/Cargo.toml" | /usr/bin/sed 's/version = "\(.*\)"/\1/')"
  git_sha="$(git -C "$repository_root" rev-parse HEAD 2>/dev/null || printf unknown)"
  if [ "$git_sha" != unknown ] && [ -n "$(git -C "$repository_root" status --porcelain 2>/dev/null)" ]; then
    git_sha="$git_sha-dirty"
  fi
  printf '{"version":"%s","git_sha":"%s"}\n' "$version" "$git_sha"
}

validate_packaged_sidecar() {
  helper="$1"
  required_archs="$2"
  expected_signing="$3"

  [ -f "$helper" ] || { echo "error: packaged sidecar missing at $helper" >&2; return 1; }
  build_identity="$($helper --build-identity 2>/dev/null)" || {
    echo "error: packaged sidecar does not expose Phoenix build identity" >&2
    return 1
  }
  printf '%s' "$build_identity" | /usr/bin/grep -Eq '^\{"version":"[^"]+","git_sha":"(unknown|[0-9a-f]{12,40}(-dirty)?)"\}$' || {
    echo "error: packaged sidecar returned invalid Phoenix build identity" >&2
    return 1
  }
  if [ -n "$expected_identity" ] && [ "$build_identity" != "$expected_identity" ]; then
    echo "error: packaged sidecar build identity does not match expected build" >&2
    return 1
  fi
  helper_archs="$(lipo -archs "$helper")"
  for arch in $required_archs; do
    case " $helper_archs " in
      *" $arch "*) ;;
      *) echo "error: packaged sidecar is missing required architecture $arch (has: $helper_archs)" >&2; return 1 ;;
    esac
  done

  case "$expected_signing" in
    none) ;;
    adhoc|identity:*)
      if ! codesign --verify --strict "$helper" >/dev/null 2>&1; then
        echo "error: packaged sidecar failed codesign --verify --strict" >&2
        return 1
      fi
      if [ "$expected_signing" = "adhoc" ]; then
        authority_count="$(codesign -dv "$helper" 2>&1 | /usr/bin/grep -c '^Authority=' || true)"
        if [ "$authority_count" -ne 0 ]; then
          echo "error: packaged sidecar unexpectedly has an Authority chain; expected ad-hoc signing" >&2
          return 1
        fi
      else
        identity="${expected_signing#identity:}"
        non_hex="$(printf '%s' "$identity" | /usr/bin/tr -d '[:xdigit:]')"
        identity_length="${#identity}"
        if [ -z "$non_hex" ] && { [ "$identity_length" -eq 40 ] || [ "$identity_length" -eq 64 ]; }; then
          designated_requirement="anchor trusted and certificate leaf = H\"$identity\""
        else
          designated_requirement="anchor trusted and certificate leaf[subject.CN] = \"$identity\""
        fi
        if ! codesign --verify --strict --requirement "$designated_requirement" "$helper" >/dev/null 2>&1; then
          echo "error: packaged sidecar is not signed with expected identity: $identity" >&2
          return 1
        fi
      fi
      ;;
  esac
}

if [ -z "${PHOENIX_SIDECAR_PATH:-}" ]; then
  if [ "${CONFIGURATION:-Debug}" = "Release" ]; then
    echo "error: Release builds require PHOENIX_SIDECAR_PATH" >&2
    exit 1
  fi
  /bin/rm -f "$destination"
  echo "Phoenix sidecar not supplied; attached mode remains available"
  exit 0
fi

if [ ! -f "$PHOENIX_SIDECAR_PATH" ] || [ ! -x "$PHOENIX_SIDECAR_PATH" ]; then
  echo "error: PHOENIX_SIDECAR_PATH must name an executable file" >&2
  exit 1
fi

expected_identity=""
if [ "${CONFIGURATION:-Debug}" = "Release" ]; then
  expected_identity="$(expected_build_identity)" || {
    echo "error: Release builds require repository-derived Phoenix build identity" >&2
    exit 1
  }
fi

mkdir -p "$helpers"
/bin/cp -f "$PHOENIX_SIDECAR_PATH" "$destination"
/bin/chmod 755 "$destination"

source_archs="$(lipo -archs "$PHOENIX_SIDECAR_PATH")"
for arch in $ARCHS; do
  case " $source_archs " in
    *" $arch "*) ;;
    *) echo "error: sidecar is missing required architecture $arch (has: $source_archs)" >&2; exit 1 ;;
  esac
done

expected_signing=none
if [ "${EXPANDED_CODE_SIGN_IDENTITY:-}" = "-" ] && [ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" ]; then
  codesign --force --sign - --options runtime --timestamp=none "$destination"
  expected_signing=adhoc
elif [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ] && [ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" ]; then
  codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" --options runtime --timestamp=none "$destination"
  expected_signing="identity:$EXPANDED_CODE_SIGN_IDENTITY"
elif [ "${AD_HOC_CODE_SIGNING_ALLOWED:-NO}" = "YES" ] && [ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" ]; then
  codesign --force --sign - --timestamp=none "$destination"
  expected_signing=adhoc
elif [ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" ] && [ "${CODE_SIGN_STYLE:-}" = "Automatic" ]; then
  codesign --force --sign - --timestamp=none "$destination"
  expected_signing=adhoc
fi

if [ "${SKIP_PACKAGE_SIDECAR_MAIN:-0}" != "1" ]; then
  validate_packaged_sidecar "$destination" "$ARCHS" "$expected_signing"
fi
