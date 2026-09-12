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
    assert worker['ProgramArguments'][-1] == profile
    assert worker['RunAtLoad'] is False and worker['KeepAlive'] is False
    assert not central_path.exists(), 'switching roles must remove the old LaunchAgent'
    assert not gateway_path.exists(), 'worker mode must remove the central memory gateway'
    settings = json.loads((home / '.config/gah/desktop.json').read_text())
    assert settings['repository_path'] == str(repo.resolve())
    assert settings['server_port'] == 4774
    assert worker_path.stat().st_mode & 0o777 == 0o644

    subprocess.run(['bash', str(source), 'install', 'worker', str(repo)], env=env, check=True)
    assert not worker_path.exists(), 'a missing profile must not reload a stale worker service'

    invalid = subprocess.run(
        ['bash', str(source), 'install', 'central', str(repo)],
        env={**env, 'GAH_DESKTOP_SERVER_PORT': '80'}, capture_output=True, text=True,
    )
    assert invalid.returncode != 0 and 'between 1024 and 65535' in invalid.stderr

print('macOS LaunchAgents passed: role exclusivity, gateway ownership, escaped values, fixed local port, and persisted checkout.')
