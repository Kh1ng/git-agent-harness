"""Run complete worker shell installers; Cargo builds and service startup remain separate acceptance tests."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

source = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix='gah-worker-install-') as temp:
    root = Path(temp)
    scripts = root / 'repo/scripts'
    scripts.mkdir(parents=True)
    for name in ['install.sh', 'install-linux.sh', 'install-macos.sh', 'configure-node-role.sh']:
        shutil.copy2(source / name, scripts / name)
    binaries = root / 'bin'
    binaries.mkdir()
    (binaries / 'python3').symlink_to(sys.executable)
    (binaries / 'uname').write_text('#!/bin/sh\nprintf "%s\\n" "$GAH_TEST_OS"\n')
    (binaries / 'uname').chmod(0o755)
    for name in ['sudo', 'npm', 'curl', 'systemctl']:
        command = binaries / name
        command.write_text('#!/bin/sh\nprintf "forbidden: %s\\n" "$0" | tee -a "$GAH_TEST_LOG" >&2\nexit 99\n')
        command.chmod(0o755)
    cargo = binaries / 'cargo'
    cargo.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ['GAH_TEST_LOG'], 'a') as log:
    log.write(json.dumps(args) + '\\n')
prefix = ['run', '--bin', 'gah', '--']
assert args[:4] == prefix, args
if args[4:6] == ['config', 'set']:
    assert args[6:] == ['--node-role', 'worker', '--registry-central-url', 'https://central.test'], args
else:
    assert args[4:6] == ['update', '--repo'] and args[7:] == ['--role', 'worker'], args
    assert pathlib.Path(args[6]).resolve() == pathlib.Path.cwd()
    credential = pathlib.Path.home() / '.config/gah/gah-loop.env'
    assert credential.is_file(), 'Worker credentials must exist before update can start services'
    assert credential.stat().st_mode & 0o777 == 0o600
''')
    cargo.chmod(0o755)
    for platform in ['Linux', 'Darwin']:
        home = root / platform
        home.mkdir()
        log = home / 'commands.jsonl'
        # Use only test settings; inherited credentials and installer options must not enter this run.
        env = {
            'HOME': str(home), 'PATH': f'{binaries}:/usr/bin:/bin',
            'GAH_TEST_OS': platform, 'GAH_TEST_LOG': str(log),
            'GAH_NODE_ROLE': 'worker', 'GAH_CENTRAL_URL': 'https://central.test',
            'COORDINATOR_TOKEN': 'installer-test-token',
        }
        result = subprocess.run(['bash', str(scripts / 'install.sh')], env=env, capture_output=True, text=True)
        assert result.returncode == 0, f'{platform}: {result.stdout}\n{result.stderr}'
        assert '/etc/gah' not in result.stdout, result.stdout
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        assert len(calls) == 2 and calls[0][4:6] == ['config', 'set'] and calls[1][4] == 'update', calls
        credential = home / '.config/gah/gah-loop.env'
        assert credential.read_text() == 'COORDINATOR_TOKEN="installer-test-token"\n'
        assert not (home / '.config/systemd').exists()
print('Worker installers passed: Linux and macOS entrypoints, config before update, private credentials, no privileged or central commands.')
