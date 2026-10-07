#!/usr/bin/env bash
# Build and atomically replace the per-user macOS desktop application.
set -euo pipefail

repo="${1:?Usage: install-macos-desktop.sh REPOSITORY}"
repo="$(cd "$repo" && pwd -P)"
app_dir="${GAH_DESKTOP_APP_DIR:-$HOME/Applications}"
target_dir="${CARGO_TARGET_DIR:-$repo/apps/desktop/target}"
case "$target_dir" in /*) ;; *) target_dir="$repo/apps/desktop/$target_dir" ;; esac
source_app="$target_dir/release/bundle/macos/GAH.app"

if [ "${GAH_DESKTOP_SKIP_BUILD:-}" != 1 ]; then
  (cd "$repo/apps/desktop" && npx vite build && npx tauri build --bundles app)
fi
[ -d "$source_app" ] || { echo "ERROR: the desktop build did not produce $source_app" >&2; exit 1; }

mkdir -p "$app_dir"
stage="$(mktemp -d "$app_dir/.gah-desktop.XXXXXX")"
backup="$app_dir/.GAH.previous.app"
next="$stage/GAH.app"
trap 'rm -rf "$stage"' EXIT
if command -v ditto >/dev/null 2>&1; then
  ditto "$source_app" "$next"
else
  cp -R "$source_app" "$next"
fi
target="$app_dir/GAH.app"
existing="$target"
if [ ! -e "$existing" ] && [ -e "$app_dir/GAH Worker.app" ]; then
  existing="$app_dir/GAH Worker.app"
fi
if [ -e "$backup" ]; then
  echo "ERROR: $backup exists from an earlier interrupted install; restore or remove it first" >&2
  exit 1
fi
if [ -e "$existing" ]; then mv "$existing" "$backup"; fi
if ! mv "$next" "$target"; then
  [ ! -e "$backup" ] || mv "$backup" "$target"
  echo "ERROR: could not install $target; the previous app was restored" >&2
  exit 1
fi
rm -rf "$backup"
echo "Installed desktop app: $target"

# Release DMGs installed the same app system-wide as "GAH Worker.app". A
# per-user install leaves that copy behind as a stale second app (#1409).
legacy_dir="${GAH_LEGACY_APP_DIR:-/Applications}"
legacy="$legacy_dir/GAH Worker.app"
if [ "$legacy_dir" != "$app_dir" ] && [ -d "$legacy" ]; then
  if rm -rf "$legacy" 2>/dev/null; then
    echo "Removed the old app: $legacy"
  else
    echo "WARNING: could not remove $legacy; delete it by hand" >&2
  fi
fi
