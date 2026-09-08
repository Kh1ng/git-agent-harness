"""Exercise the installer's generated config without installing a service."""
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile
from unittest.mock import patch

script = (Path(__file__).parent / 'install-wsl-worker.sh').read_text()
code = script.split("<<'PY'\n", 1)[1].split('\nPY\n', 1)[0]
with tempfile.TemporaryDirectory(prefix='gah-wsl-check-') as temp:
    home = Path(temp) / "home with 'quotes %"
    home.mkdir()
    for directory in ['.local/bin', '.opencode/bin']:
        (home / directory).mkdir(parents=True)
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
    assert 'GAH_NODE_ROLE' not in env  # Role comes from config after a service restart.
    assert env['GAH_CONFIG_PATH'] == str(home / '.config/gah/config.toml')
    assert env['GAH_BINARY'] == str(release / 'bin/gah')
    for directory in ['.local/bin', '.opencode/bin']:
        assert str(home / directory) in env['PATH'].split(':')
    unit = (home / '.config/systemd/user/gah-worker.service').read_text()
    assert 'Restart=on-failure' in unit and '%%' in unit
    assert token not in unit and token not in (root / 'register.sh').read_text()
print('WSL config check passed: private credentials, shell quoting, stable identity, service paths.')

# An old downloaded CLI must fail before the installer rewrites live worker files.
role_check = script.split('install -m 755 "$stage/gah" "$release_dir/bin/gah"\n', 1)[1].split('# role-cli-check:end', 1)[0]
assert script.index('# role-cli-check:end') < script.index("<<'PY'\n")
with tempfile.TemporaryDirectory(prefix='gah-role-install-') as temp:
    root = Path(temp)
    (root / 'bin').mkdir()
    cli = root / 'bin/gah'
    cli.write_text('#!/bin/sh\necho "old CLI help"\n')
    cli.chmod(0o755)
    result = subprocess.run(['bash', '-ec', role_check], env={**os.environ, 'release_dir': str(root)}, capture_output=True, text=True)
    assert result.returncode != 0 and 'too old' in result.stderr
    cli.write_text('#!/bin/sh\ncase "$*" in "config set --help"|"status --help") echo "--node-role --role";; *) exit 2;; esac\n')
    assert subprocess.run(['bash', '-ec', role_check], env={**os.environ, 'release_dir': str(root)}).returncode == 0

    cli.write_text('#!/bin/sh\necho "--node-role --role"\n')
    # Shared Linux/macOS bootstrap preserves other env settings and safely quotes secrets.
    home = root / 'home'
    home.mkdir()
    env_file = home / '.config/gah/gah-loop.env'
    env_file.parent.mkdir(parents=True)
    env_file.write_text('OTHER=preserved\n')
    configure = Path(__file__).parent / 'configure-node-role.sh'
    env = {**os.environ, 'HOME': str(home), 'COORDINATOR_TOKEN': token, 'GAH_CENTRAL_URL': 'https://central.test'}
    env.pop('GAH_GATEWAY_MODE', None)
    subprocess.run(['bash', str(configure), 'worker', str(cli)], env=env, check=True, capture_output=True)
    output = subprocess.check_output(['bash', '-c', 'set -a; source "$1"; exec "$2" -c \'import os; print(os.environ["COORDINATOR_TOKEN"])\'', 'check', str(env_file), sys.executable], text=True)
    assert output.rstrip('\n') == token
    assert env_file.stat().st_mode & 0o777 == 0o600
    assert 'OTHER=preserved' in env_file.read_text()
    saved = env_file.read_text()
    del env['COORDINATOR_TOKEN']
    subprocess.run(['bash', str(configure), 'worker', str(cli)], env=env, check=True, capture_output=True)
    assert env_file.read_text() == saved
    env['GAH_GATEWAY_MODE'] = 'remote'
    assert subprocess.run(['bash', str(configure), 'worker', str(cli)], env=env, capture_output=True).returncode != 0
print('Role bootstrap passed: old CLI rejected, private worker credentials, existing settings preserved.')

# Exercise the real bootstrap blocks without apt, downloads, or installing a service.
packages = script.split('\nfi\n', 1)[1].split('install_dir=', 1)[0]
runtime = script.split('# role-cli-check:end\n', 1)[1].split('\ncd "$release_dir"', 1)[0]
with tempfile.TemporaryDirectory(prefix='gah-wsl-runtime-') as temp:
    root = Path(temp)
    binaries = root / 'bin'
    native = root / 'native node/bin'
    binaries.mkdir()
    native.mkdir(parents=True)
    log = root / 'commands'

    def executable(path, content):
        path.write_text('#!/bin/sh\n' + content)
        path.chmod(0o755)

    executable(binaries / 'dpkg-query', 'for package; do :; done\n[ "$package" != "$GAH_TEST_MISSING" ] && printf "installed"\n')
    executable(binaries / 'sudo', 'printf "%s\n" "$*" >> "$GAH_TEST_LOG"\n')
    env = {**os.environ, 'PATH': f'{binaries}:/usr/bin:/bin', 'GAH_TEST_LOG': str(log), 'GAH_TEST_MISSING': ''}
    result = subprocess.run(['bash', '-euc', packages], env=env, capture_output=True, text=True)
    assert result.returncode == 0 and not log.exists(), 'Installed prerequisites must not require sudo'
    env['GAH_TEST_MISSING'] = 'xz-utils'
    result = subprocess.run(['bash', '-euc', packages], env=env, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert log.read_text().splitlines() == ['apt-get update', 'apt-get install -y xz-utils']
    log.unlink()

    # A real JS evaluator supplies Linux metadata while the test also runs on macOS CI.
    real_node = shutil.which('node')
    assert real_node, 'Node is required by the WSL installer regression test'
    executable(native / 'node', """exec "$GAH_TEST_REAL_NODE" -e 'Object.defineProperty(process, "platform", {value: process.env.GAH_TEST_PLATFORM}); Object.defineProperty(process, "execPath", {value: process.env.GAH_TEST_EXEC_PATH}); Object.defineProperty(process.versions, "node", {value: process.env.GAH_TEST_NODE_VERSION}); console.log(eval(process.argv[1]))' "$2"
""")
    (binaries / 'node').symlink_to(native / 'node')
    executable(native / 'npm', 'printf "linux-npm\\n"\n')
    executable(binaries / 'npm', 'echo "Windows npm selected" >&2; exit 99\n')
    executable(binaries / 'curl', 'echo download >> "$GAH_TEST_LOG"; exit 71\n')
    env.update(GAH_TEST_REAL_NODE=real_node, GAH_TEST_EXEC_PATH=str(native / 'node'),
               GAH_TEST_PLATFORM='linux', GAH_TEST_NODE_VERSION='22.22.3', install_dir=str(root), release_dir=str(root))
    result = subprocess.run(['bash', '-euc', runtime + '\nnpm --version\nprintf "%s" "$node"'], env=env, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout == f'linux-npm\n{native / "node"}', result.stdout
    assert not log.exists(), 'Compatible Linux Node and npm must be reused'
    for platform, version, has_npm in [('win32', '22.22.3', True), ('linux', '18.0.0', True), ('linux', '22.22.3', False)]:
        env.update(GAH_TEST_PLATFORM=platform, GAH_TEST_NODE_VERSION=version)
        if not has_npm:
            (native / 'npm').unlink()
        result = subprocess.run(['bash', '-euc', runtime], env=env, capture_output=True, text=True)
        assert result.returncode == 71 and log.read_text().strip() == 'download', (platform, version, has_npm, result.stderr)
        log.unlink()
print('WSL bootstrap passed: conditional apt, resolved Linux Node/npm, incompatible runtime fallback, agent PATH.')
