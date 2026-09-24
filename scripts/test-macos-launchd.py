"""Exercise generated LaunchAgents without loading services on the host."""
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile

source = Path(__file__).with_name('macos-launchd.sh')
with tempfile.TemporaryDirectory(prefix='gah-launchd-') as temporary:
    root = Path(temporary)
    home = root / 'home'
    repo = root / 'repo'
    agents = home / 'agents'
    (repo / 'apps/server/dist').mkdir(parents=True)
    (repo / 'apps/web/dist').mkdir(parents=True)
    (repo / 'Cargo.toml').write_text('[package]\nname="fixture"\nversion="0.0.0"\n')
    (repo / 'apps/server/dist/bin.js').write_text('')
    (repo / 'apps/web/dist/index.html').write_text('')
    memory = root / 'MemoryCore'
    (memory / 'src/gateway').mkdir(parents=True)
    (memory / 'src/gateway/server.ts').write_text('')
    (memory / 'tdai-gateway.local.yaml').write_text('')
    (home / '.config/gah').mkdir(parents=True)
    (home / '.config/gah/tdai-gateway.env').write_text('TDAI_GATEWAY_API_KEY="test"\n')
    env = {
        **os.environ,
        'HOME': str(home),
        'GAH_LAUNCHD_DRY_RUN': '1',
        'GAH_LAUNCH_AGENTS_DIR': str(agents),
        'GAH_LAUNCHD_UID': '501',
        'GAH_NODE_PATH': '/opt/homebrew/bin/node',
        'GAH_CLI_PATH': '/Users/test/.cargo/bin/gah',
        'GAH_NPX_PATH': '/opt/homebrew/bin/npx',
        'GAH_GATEWAY_MEMORYCORE_PATH': str(memory),
        'GAH_DESKTOP_SERVER_PORT': '4774',
        'GAH_NODE_ADVERTISED_URL': 'https://mac.test.ts.net:4774',
        'GAH_TAILSCALE_PATH': '/Applications/Tailscale.app/Contents/MacOS/Tailscale',
    }
    subprocess.run(['bash', str(source), 'install', 'central', str(repo)], env=env, check=True)
    central_path = agents / 'dev.git-agent-harness.server.plist'
    central = plistlib.loads(central_path.read_bytes())
    assert central['Label'] == 'dev.git-agent-harness.server'
    assert central['ProgramArguments'][-1] == str(repo.resolve() / 'apps/server/dist/bin.js')
    assert central['EnvironmentVariables']['GAH_WEB_ROOT'] == str(repo.resolve() / 'apps/web/dist')
    assert central['EnvironmentVariables']['PORT'] == '4774'
    assert central['RunAtLoad'] is True and central['KeepAlive'] is True
    gateway_path = agents / 'dev.git-agent-harness.memory-gateway.plist'
    gateway = plistlib.loads(gateway_path.read_bytes())
    assert gateway['WorkingDirectory'] == str(memory.resolve())
    assert gateway['ProgramArguments'][-1] == '/opt/homebrew/bin/npx'

    profile = 'répo<&>'
    subprocess.run(['bash', str(source), 'install', 'worker', str(repo), profile], env=env, check=True)
    worker_path = agents / 'dev.git-agent-harness.worker.plist'
    worker = plistlib.loads(worker_path.read_bytes())
    assert worker['ProgramArguments'][-2] == str(repo.resolve() / 'apps/server/dist/bin.js')
    assert worker['EnvironmentVariables']['HOST'] == '127.0.0.1'
    assert worker['EnvironmentVariables']['PORT'] == '4774'
    assert worker['EnvironmentVariables']['GAH_BINARY'] == '/Users/test/.cargo/bin/gah'
    assert worker['EnvironmentVariables']['GAH_REGISTRY_TRANSPORT_MODE'] == 'authenticated_remote'
    assert worker['EnvironmentVariables']['GAH_TAILSCALE_SERVE'] == '1'
    identity_path = home / '.local/share/gah/worker/identity.json'
    assert worker['EnvironmentVariables']['GAH_COORDINATOR_IDENTITY_PATH'] == str(identity_path)
    identity = json.loads(identity_path.read_text())
    assert identity['advertised_url'] == 'https://mac.test.ts.net:4774'
    assert identity_path.stat().st_mode & 0o777 == 0o600
    assert worker['RunAtLoad'] is False and worker['KeepAlive'] is True
    assert not central_path.exists(), 'switching roles must remove the old LaunchAgent'
    assert not gateway_path.exists(), 'worker mode must remove the central memory gateway'
    settings = json.loads((home / '.config/gah/desktop.json').read_text())
    assert settings['repository_path'] == str(repo.resolve())
    assert settings['server_port'] == 4774
    assert worker_path.stat().st_mode & 0o777 == 0o644

    subprocess.run(['bash', str(source), 'install', 'worker', str(repo)], env=env, check=True)
    assert worker_path.exists(), 'a fresh worker must run before its first profile is imported'

    tailscale = root / 'tailscale'
    tailscale.write_text('#!/bin/sh\nprintf \'%s\\n\' \'{"Self":{"DNSName":"mac.test.ts.net.","TailscaleIPs":["100.64.0.42","fd7a:115c:a1e0::1"]}}\'\n')
    tailscale.chmod(0o700)
    default_transport_env = {
        key: value for key, value in env.items() if key != 'GAH_NODE_ADVERTISED_URL'
    }
    default_transport_env['GAH_TAILSCALE_PATH'] = str(tailscale)
    subprocess.run(
        ['bash', str(source), 'install', 'worker', str(repo), profile],
        env=default_transport_env,
        check=True,
    )
    worker = plistlib.loads(worker_path.read_bytes())
    identity = json.loads(identity_path.read_text())
    assert identity['advertised_url'] == 'http://100.64.0.42:4774'
    assert worker['EnvironmentVariables']['HOST'] == '100.64.0.42'
    assert worker['EnvironmentVariables']['GAH_REGISTRY_TRANSPORT_MODE'] == 'trusted_lan'
    assert worker['EnvironmentVariables']['GAH_TAILSCALE_SERVE'] == '0'

    invalid = subprocess.run(
        ['bash', str(source), 'install', 'central', str(repo)],
        env={**env, 'GAH_DESKTOP_SERVER_PORT': '80'}, capture_output=True, text=True,
    )
    assert invalid.returncode != 0 and 'between 1024 and 65535' in invalid.stderr

print('macOS LaunchAgents passed: role exclusivity, worker server identity, gateway ownership, fixed port, and persisted checkout.')
