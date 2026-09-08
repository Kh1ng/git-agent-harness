#!/usr/bin/env bash
# Persist the installer's role and worker credential before services can restart.
# Remaining arguments select the current CLI (including cargo run on a fresh host).
set -euo pipefail
role="${1:?Missing node role}"
shift
if [ "$role" = worker ]; then
  if [ -n "${GAH_GATEWAY_MODE:-}" ]; then
    echo 'ERROR: workers use the central memory relay. Configure GAH_CENTRAL_URL and COORDINATOR_TOKEN; configure GAH_GATEWAY_MODE on central only.' >&2
    exit 1
  fi
  if [ -n "${GAH_IMPORT_REPO:-}" ]; then
    echo 'ERROR: import shared project memory on the central node.' >&2
    exit 1
  fi
  python3 - <<'PY'
import os
from pathlib import Path
path = Path.home() / '.config/gah/gah-loop.env'
token = os.environ.get('COORDINATOR_TOKEN', '')
if token:
    if not token.strip() or any(ord(c) < 32 or ord(c) == 127 for c in token):
        raise SystemExit('ERROR: COORDINATOR_TOKEN must not contain control characters.')
elif not path.exists() or not any(line.startswith('COORDINATOR_TOKEN=') and line.partition('=')[2].strip(" '\"") for line in path.read_text().splitlines()):
    raise SystemExit('ERROR: worker installation requires COORDINATOR_TOKEN for central claims and memory.')
PY
fi
args=(config set --node-role "$role")
if [ -n "${GAH_CENTRAL_URL:-}" ]; then args+=(--registry-central-url "$GAH_CENTRAL_URL"); fi
"$@" "${args[@]}"
if [ "$role" = worker ] && [ -n "${COORDINATOR_TOKEN:-}" ]; then
  python3 - <<'PY'
import os
from pathlib import Path
path = Path.home() / '.config/gah/gah-loop.env'
path.parent.mkdir(parents=True, exist_ok=True)
lines = path.read_text().splitlines() if path.exists() else []
lines = [line for line in lines if not line.startswith('COORDINATOR_TOKEN=')]
token = os.environ['COORDINATOR_TOKEN']
for character in ['\\', '"', '$', '`']:
    token = token.replace(character, '\\' + character)
lines.append('COORDINATOR_TOKEN="' + token + '"')
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.fchmod(fd, 0o600)
with os.fdopen(fd, 'w') as file:
    file.write('\n'.join(lines) + '\n')
PY
fi
