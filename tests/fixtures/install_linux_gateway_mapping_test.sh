#!/usr/bin/env bash
# Exercises the real central/worker gateway-target mapping, extracted
# verbatim from scripts/install-linux.sh between the
# gateway-target-mapping:start/:end markers, against a scratch HOME. Run by
# tests/install_linux_gateway_targets.rs. This never touches a live host's
# /etc/gah or ~/.config/gah -- every path below is redirected into a temp
# dir, and `sudo` is stubbed to exec in place (no real privilege escalation)
# so central's server.env target can be written without root.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
install_script="$repo_root/scripts/install-linux.sh"

fragment="$(sed -n '/gateway-target-mapping:start/,/gateway-target-mapping:end/p' "$install_script" | sed '1d;$d')"
if [ -z "$fragment" ]; then
  echo "FAIL: could not extract gateway-target-mapping block from $install_script (markers moved/renamed?)" >&2
  exit 1
fi

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

stub_bin="$work_dir/bin"
mkdir -p "$stub_bin"
cat > "$stub_bin/sudo" <<'EOF'
#!/usr/bin/env bash
exec "$@"
EOF
chmod +x "$stub_bin/sudo"
PATH="$stub_bin:$PATH"
export PATH

fail=0

check_role() {
  local role="$1" expect_server_written="$2" label="$3"
  local server_env_file="$work_dir/$role-server.env"
  local HOME="$work_dir/$role-home"
  mkdir -p "$HOME"

  eval "$fragment"
  upsert_gateway_env_line TDAI_GATEWAY_URL "http://example.invalid:8420"
  upsert_gateway_env_line TDAI_GATEWAY_API_KEY "test-key-$role"

  local loop_env_file="$HOME/.config/gah/gah-loop.env"
  if [ ! -f "$loop_env_file" ] \
    || ! grep -q '^TDAI_GATEWAY_URL=http://example.invalid:8420$' "$loop_env_file" \
    || ! grep -q "^TDAI_GATEWAY_API_KEY=test-key-$role\$" "$loop_env_file"; then
    echo "FAIL ($label): $loop_env_file missing expected keys" >&2
    fail=1
  fi

  if [ "$expect_server_written" = "yes" ]; then
    if [ ! -f "$server_env_file" ] \
      || ! grep -q '^TDAI_GATEWAY_URL=http://example.invalid:8420$' "$server_env_file" \
      || ! grep -q "^TDAI_GATEWAY_API_KEY=test-key-$role\$" "$server_env_file"; then
      echo "FAIL ($label): $server_env_file missing expected keys" >&2
      fail=1
    fi
  elif [ -f "$server_env_file" ]; then
    echo "FAIL ($label): $server_env_file should not exist for role=$role" >&2
    fail=1
  fi
}

check_role central yes "central writes both targets"
check_role worker no "worker writes only its own target"

if [ "$fail" != "0" ]; then
  exit 1
fi
echo "OK: install-linux.sh gateway target mapping writes the correct files per role"
