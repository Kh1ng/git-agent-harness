"""Exercise the installer's generated config without installing a service."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

script = (Path(__file__).parent / 'install-wsl-worker.sh').read_text()
code = script.split("<<'PY'\n", 1)[1].split('\nPY\n', 1)[0]
with tempfile.TemporaryDirectory(prefix='gah-wsl-check-') as temp:
    home = Path(temp) / "home with 'quotes %"
    home.mkdir()
    root = home / '.local/share/gah/worker'
    root.mkdir(parents=True)
    release = root / 'release.test'
    release.mkdir()
    settings = root / 'settings.json'
    token = "secret'$dollar;still-data"
    settings.write_text(json.dumps({'token': token, 'central_url': 'http://192.168.1.10:3773', 'display_name': 'Test Windows', 'advertised_url': 'http://192.168.1.11:3774'}))
    args = ['install', str(settings), str(root), str(release), sys.executable]
    with patch.object(Path, 'home', return_value=home), patch.object(sys, 'argv', args):
        exec(compile(code, 'install-wsl-worker.sh:python', 'exec'), {})
    identity = json.loads((root / 'identity.json').read_text())
    with patch.object(Path, 'home', return_value=home), patch.object(sys, 'argv', args):
        exec(compile(code, 'install-wsl-worker.sh:python', 'exec'), {})
    assert json.loads((root / 'identity.json').read_text())['node_id'] == identity['node_id']
    assert (root / 'worker.env').stat().st_mode & 0o777 == 0o600
    for name in ['worker.env', 'start.sh', 'register.sh']:
        subprocess.run(['bash', '-n', str(root / name)], check=True)
    output = subprocess.check_output(['bash', '-c', 'source "$1"; exec "$2" -c \'import os,json; print(json.dumps(dict(os.environ)))\'', 'check', str(root / 'worker.env'), sys.executable], text=True)
    env = json.loads(output)
    assert env['COORDINATOR_TOKEN'] == token
    assert env['GAH_NODE_ROLE'] == 'worker'
    assert env['GAH_CONFIG_PATH'] == str(home / '.config/gah/config.toml')
    assert env['GAH_BINARY'] == str(release / 'bin/gah')
    unit = (home / '.config/systemd/user/gah-worker.service').read_text()
    assert 'Restart=on-failure' in unit and '%%' in unit
    assert token not in unit and token not in (root / 'register.sh').read_text()
print('WSL config check passed: private credentials, shell quoting, stable identity, service paths.')
