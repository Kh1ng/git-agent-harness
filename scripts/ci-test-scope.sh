#!/usr/bin/env bash
# Read NUL-separated changed paths from git diff and select complete suites.
set -euo pipefail
has_rust_changes=false
has_desktop_changes=false
while IFS= read -r -d '' path; do
  case "$path" in
    .github/workflows/CI.yml|scripts/ci-test-scope.sh|scripts/test-ci-test-scope.sh)
      has_rust_changes=true
      has_desktop_changes=true
      ;;
    Cargo.toml|Cargo.lock|build.rs|.cargo/*|src/*|tests/*|scripts/*)
      has_rust_changes=true
      ;;
    apps/desktop/*)
      has_desktop_changes=true
      ;;
  esac
done
printf 'has_rust_changes=%s\nhas_desktop_changes=%s\n' "$has_rust_changes" "$has_desktop_changes"
