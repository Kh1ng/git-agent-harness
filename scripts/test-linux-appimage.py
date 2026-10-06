#!/usr/bin/env python3
"""Inspect a real Linux release AppImage, never a browser fixture.

Run inside dbus-run-session and (for headless validation) xvfb-run. The caller
must provide an empty evidence directory. Exit 1 means failed, 2 unsupported.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import shutil
import subprocess
import tempfile
import time


REQUIRED_TEXT = (
    'Settings · This computer',
    'Set up this computer',
    'GAH is not installed on this computer yet.',
)


def walk(node):
    yield node
    for index in range(node.childCount):
        child = node.getChildAtIndex(index)
        if child is not None:
            yield from walk(child)


def onboarding(nodes, atspi):
    """Require the web document, real fresh-install IPC result and usable action."""
    texts = []
    documents = []
    actions = []
    for node in nodes:
        texts.append(node.name or '')
        try:
            text = node.queryText()
            texts.append(text.getText(0, text.characterCount))
        except NotImplementedError:
            pass
        states = node.getState()
        if node.getRoleName() in ('document web', 'document frame'):
            if states.contains(atspi.STATE_SHOWING):
                documents.append(node)
        if node.getRoleName() == 'push button' and node.name == 'Set up standalone':
            if states.contains(atspi.STATE_ENABLED) and states.contains(atspi.STATE_SENSITIVE):
                actions.append(node)
    combined = '\n'.join(texts)
    matched = [text for text in REQUIRED_TEXT if text in combined]
    return {
        'matched_text': matched,
        'showing_web_document': bool(documents),
        'enabled_standalone_action': bool(actions),
        'passed': len(matched) == len(REQUIRED_TEXT) and bool(documents) and bool(actions),
    }, actions


def artifact_sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def screenshot(evidence, name, env, pid):
    windows = subprocess.check_output(
        ['xdotool', 'search', '--all', '--onlyvisible', '--pid', str(pid), '--name', '^GAH$'],
        env=env, text=True, timeout=15).splitlines()
    if len(windows) != 1:
        raise RuntimeError('Expected exactly one visible GAH window')
    subprocess.run(['import', '-window', windows[0], str(evidence / name)],
                   env=env, check=True, timeout=15)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('artifact', type=Path)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--environment', choices=('headless', 'interactive'), required=True)
    parser.add_argument('--timeout', type=float, default=60)
    args = parser.parse_args()
    artifact = args.artifact.resolve(strict=True)
    args.evidence.mkdir(parents=True, exist_ok=True)
    if any(args.evidence.iterdir()):
        parser.error('evidence directory must be empty')
    evidence = args.evidence.resolve()
    report = {
        'artifact': artifact.name,
        'sha256': artifact_sha256(artifact),
        'environment': args.environment,
        'platform': platform.platform(),
        'session_type': os.environ.get('XDG_SESSION_TYPE', 'unknown'),
        'software_rendering': os.environ.get('LIBGL_ALWAYS_SOFTWARE', 'unset'),
        'dmabuf_disabled': os.environ.get('WEBKIT_DISABLE_DMABUF_RENDERER', 'unset'),
        'status': 'unsupported',
    }
    process = None
    try:
        if platform.system() != 'Linux' or not os.environ.get('DISPLAY'):
            raise RuntimeError('Linux with an X11 display is required')
        if args.environment == 'interactive' and not os.environ.get('GAH_INTERACTIVE_DESKTOP_CONFIRMED') == '1':
            raise RuntimeError('Interactive mode requires a human-attended desktop and GAH_INTERACTIVE_DESKTOP_CONFIRMED=1')
        if not os.environ.get('DBUS_SESSION_BUS_ADDRESS'):
            raise RuntimeError('A session D-Bus connection is required')
        for command in ('import', 'xdotool', 'tesseract'):
            if shutil.which(command, path='/usr/bin:/bin') is None:
                raise RuntimeError(f'Missing test dependency: {command}')
        import pyatspi  # Ubuntu package python3-pyatspi, system Python required.
        report['status'] = 'failed'
        with tempfile.TemporaryDirectory(prefix='gah-appimage-smoke-') as scratch:
            root = Path(scratch)
            # Do not inherit provider credentials, CLI paths or the developer's config.
            env = {key: os.environ[key] for key in (
                'DISPLAY', 'XAUTHORITY', 'DBUS_SESSION_BUS_ADDRESS', 'LANG',
                'LIBGL_ALWAYS_SOFTWARE', 'WEBKIT_DISABLE_DMABUF_RENDERER',
            ) if key in os.environ}
            env.update(PATH='/usr/bin:/bin', HOME=str(root / 'home'),
                       XDG_CONFIG_HOME=str(root / 'config'), XDG_DATA_HOME=str(root / 'data'),
                       XDG_CACHE_HOME=str(root / 'cache'), XDG_RUNTIME_DIR=str(root / 'runtime'),
                       NO_AT_BRIDGE='0', GTK_MODULES='gail:atk-bridge')
            for directory in ('home', 'config', 'data', 'cache', 'runtime'):
                (root / directory).mkdir(mode=0o700)
            with (evidence / 'extract.log').open('w') as log:
                subprocess.run([str(artifact), '--appimage-extract'], cwd=root, env=env,
                               stdout=log, stderr=subprocess.STDOUT, check=True, timeout=60)
            appdir = root / 'squashfs-root'
            report['extracted_entrypoint'] = (appdir / 'AppRun').is_file()
            with (evidence / 'runtime.log').open('w') as log:
                process = subprocess.Popen([str(appdir / 'AppRun')], cwd=appdir, env=env,
                                           stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                deadline = time.monotonic() + args.timeout
                actions = []
                while time.monotonic() < deadline and process.poll() is None:
                    try:
                        # Only our isolated process; unrelated desktop applications cannot pass.
                        desktop = pyatspi.Registry.getDesktop(0)
                        apps = [desktop.getChildAtIndex(index) for index in range(desktop.childCount)]
                        apps = [app for app in apps if app is not None and app.get_process_id() == process.pid]
                        result, actions = onboarding([node for app in apps for node in walk(app)], pyatspi)
                        report.update({key: value for key, value in result.items() if key != 'passed'})
                        report['accessibility_passed'] = result['passed']
                        if result['passed']:
                            break
                    except Exception as error:
                        report['accessibility_error'] = type(error).__name__
                    time.sleep(0.5)
                report['exit_code_before_cleanup'] = process.poll()
                screenshot(evidence, 'initial.png', env, process.pid)
                if report.get('accessibility_passed'):
                    # Capture the actionable setup section even when it is below the fold.
                    if not actions[0].queryComponent().scrollTo(pyatspi.SCROLL_ANYWHERE):
                        raise RuntimeError('Cannot scroll setup action into view')
                    time.sleep(0.5)
                    screenshot(evidence, 'onboarding.png', env, process.pid)
                    # Accessibility alone can survive a compositor/paint failure.
                    pixels = subprocess.check_output(
                        ['tesseract', str(evidence / 'onboarding.png'), 'stdout', '--psm', '11'],
                        env=env, text=True, timeout=15)
                    report['painted_setup_action'] = 'set up standalone' in pixels.lower()
                    if not report['painted_setup_action']:
                        raise RuntimeError('Setup action is absent from screenshot OCR')
                    report['status'] = 'passed'
    except Exception as error:
        # Isolated app logs contain no real configuration or inherited provider secrets.
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        if process is not None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=5)
                # Also terminate WebKit helpers if the main process exited first.
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
        report['passed'] = report['status'] == 'passed'
        (evidence / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    return {'passed': 0, 'failed': 1, 'unsupported': 2}[report['status']]


if __name__ == '__main__':
    raise SystemExit(main())
