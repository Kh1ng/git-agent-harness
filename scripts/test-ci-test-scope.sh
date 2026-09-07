#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
check() {
  local expected actual
  expected=$(printf 'has_rust_changes=%s\nhas_desktop_changes=%s' "$1" "$2")
  shift 2
  actual=$(printf '%s\0' "$@" | bash scripts/ci-test-scope.sh)
  if [ "$actual" != "$expected" ]; then
    printf 'Wrong suite selection for %s\nExpected:\n%s\nActual:\n%s\n' "$*" "$expected" "$actual" >&2
    exit 1
  fi
}
check true false src/routing/decision.rs
check true false src/context.rs tests/gah_cli/dispatch/operator_pins.rs
check true false Cargo.lock
check true false scripts/install-macos.sh
check false true apps/desktop/main.rs
check true true src/context.rs apps/desktop/main.rs
check true true .github/workflows/CI.yml
check true true scripts/ci-test-scope.sh
check false false apps/web/src/pages/SettingsPage.tsx docs/README.md
check false false
printf 'CI suite selection: 10 cases passed\n'
