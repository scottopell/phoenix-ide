#!/bin/sh
set -eu

if [ -z "${PHOENIX_SIDECAR_PATH:-}" ]; then
  if [ "${CONFIGURATION:-Debug}" = "Release" ]; then
    echo "error: Release builds require PHOENIX_SIDECAR_PATH" >&2
    exit 1
  fi
  echo "Phoenix sidecar not supplied; attached mode remains available"
  exit 0
fi

if [ ! -f "$PHOENIX_SIDECAR_PATH" ] || [ ! -x "$PHOENIX_SIDECAR_PATH" ]; then
  echo "error: PHOENIX_SIDECAR_PATH must name an executable file" >&2
  exit 1
fi

helpers="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Helpers"
mkdir -p "$helpers"
destination="$helpers/phoenix_ide"
/bin/cp -f "$PHOENIX_SIDECAR_PATH" "$destination"
/bin/chmod 755 "$destination"

if [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ] && [ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" ]; then
  /usr/bin/codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" --options runtime --timestamp=none "$destination"
fi

source_archs="$(/usr/bin/lipo -archs "$PHOENIX_SIDECAR_PATH")"
for arch in $ARCHS; do
  case " $source_archs " in
    *" $arch "*) ;;
    *) echo "error: sidecar is missing required architecture $arch (has: $source_archs)" >&2; exit 1 ;;
  esac
done
