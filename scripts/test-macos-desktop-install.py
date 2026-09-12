"""Exercise the desktop app swap without building or touching Applications."""
import os
from pathlib import Path
import subprocess
import tempfile

source = Path(__file__).with_name('install-macos-desktop.sh')
with tempfile.TemporaryDirectory(prefix='gah-desktop-install-') as temporary:
    root = Path(temporary)
    repo = root / 'repo'
    built = repo / 'apps/desktop/target/release/bundle/macos/GAH.app'
    built.mkdir(parents=True)
    (built / 'version.txt').write_text('new')
    apps = root / 'Applications'
    installed = apps / 'GAH.app'
    legacy = apps / 'GAH Worker.app'
    legacy.mkdir(parents=True)
    (legacy / 'version.txt').write_text('old')
    subprocess.run(
        ['bash', str(source), str(repo)],
        env={**os.environ, 'HOME': str(root / 'home'), 'GAH_DESKTOP_APP_DIR': str(apps), 'GAH_DESKTOP_SKIP_BUILD': '1'},
        check=True,
    )
    assert (installed / 'version.txt').read_text() == 'new'
    assert not legacy.exists(), 'the old product name must not leave a second app'
    assert not list(apps.glob('.gah-desktop.*')), 'staging directories must be removed'
    assert not (apps / '.GAH.previous.app').exists(), 'successful installs must remove the backup'

print('macOS desktop install passed: the new app replaced the old app and staging was removed.')
