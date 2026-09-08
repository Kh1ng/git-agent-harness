#!/usr/bin/env bash
# Real first install on a disposable GitHub-hosted macOS account, never a developer's Mac.
set -euo pipefail
if [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ] || [ "$(uname -s)" != Darwin ]; then
  echo 'ERROR: this test installs GAH and may only run on a GitHub-hosted macOS runner.' >&2
  exit 1
fi
if [ -e "$HOME/.config/gah" ] || [ -e "${CARGO_HOME:-$HOME/.cargo}/bin/gah" ]; then
  echo 'ERROR: expected a fresh runner without an existing GAH installation.' >&2
  exit 1
fi

# update requires a clean default-branch checkout and performs a real fetch/pull.
# A local origin pins those operations to this PR, instead of installing main.
stage="$(mktemp -d "$RUNNER_TEMP/gah-install.XXXXXX")"
git init --bare --initial-branch=main "$stage/origin.git"
git -C "$stage/origin.git" config receive.shallowUpdate true
git push "$stage/origin.git" HEAD:refs/heads/main
git clone "$stage/origin.git" "$stage/checkout"
export CARGO_TARGET_DIR="$GITHUB_WORKSPACE/target"
export GAH_NODE_ROLE=worker
export GAH_CENTRAL_URL=https://central.example.test
export COORDINATOR_TOKEN=ci-install-test-token
bash "$stage/checkout/scripts/install-macos.sh"

"${CARGO_HOME:-$HOME/.cargo}/bin/gah" --help >/dev/null
python3 - <<'PY'
import os
from pathlib import Path
import tomllib

config_root = Path.home() / '.config'
config = tomllib.loads((config_root / 'gah/config.toml').read_text())
assert config['node_role'] == 'worker', config
assert config['registry_central_url'] == os.environ['GAH_CENTRAL_URL'], config
credentials = config_root / 'gah/gah-loop.env'
assert credentials.stat().st_mode & 0o777 == 0o600
assert credentials.read_text() == 'COORDINATOR_TOKEN="ci-install-test-token"\n'
for name in ['gah-reviewer.md', 'gah-implementer.md']:
    assert (config_root / 'opencode/agents' / name).is_file(), name
assert not (config_root / 'systemd').exists(), 'macOS must not install systemd units'
assert not Path('/etc/gah/server.env').exists(), 'a worker must not configure a central server'
print('Real macOS install passed: executable, worker role, relay URL, private credentials, agent configs, no central service.')
PY
