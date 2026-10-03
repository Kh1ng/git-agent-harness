#!/usr/bin/env bash
# Publishable macOS bundles require Developer ID signing and stapled notarization.
set -euo pipefail
for name in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [ -z "${!name:-}" ]; then
    echo "Missing repository secret: $name" >&2
    exit 1
  fi
done
case "$APPLE_SIGNING_IDENTITY" in
  'Developer ID Application: '*) ;;
  *) echo 'APPLE_SIGNING_IDENTITY must be a Developer ID Application identity.' >&2; exit 1 ;;
esac

cd "$(dirname "$0")/../apps/desktop"
npx tauri build --bundles dmg
shopt -s nullglob
apps=(target/release/bundle/macos/*.app)
dmgs=(target/release/bundle/dmg/*.dmg)
if [ "${#apps[@]}" -ne 1 ] || [ "${#dmgs[@]}" -ne 1 ]; then
  echo 'Expected exactly one signed app and one DMG.' >&2
  exit 1
fi
codesign --verify --deep --strict "${apps[0]}"
spctl --assess --type execute "${apps[0]}"
xcrun stapler validate "${apps[0]}"
# Tauri signs the DMG, but notarizes the application. Staple a ticket to the
# disk image too, so a downloaded image can be checked without network access.
codesign --verify --strict "${dmgs[0]}"
xcrun notarytool submit "${dmgs[0]}" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
xcrun stapler staple "${dmgs[0]}"
xcrun stapler validate "${dmgs[0]}"
spctl --assess --type open --context context:primary-signature "${dmgs[0]}"
