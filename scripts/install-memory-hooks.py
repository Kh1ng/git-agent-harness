"""Installer embedded in `gah setup memory-hooks`; never invokes installed hooks."""
import copy
import fcntl
import io
import json
import os
import re
from pathlib import Path
import shlex
import sys
import tempfile
from urllib.parse import urlsplit


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate JSON key; repair the file before setup')
        result[key] = value
    return result


def read_file(path):
    if path.is_symlink():
        raise ValueError(f'Refusing to replace symlink: {path}')
    if path.exists() and not path.is_file():
        raise ValueError(f'Expected a regular file: {path}')
    return path.read_bytes() if path.exists() else None


def atomic_write(path, content, mode):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as pending:
        name = pending.name
        try:
            os.fchmod(pending.fileno(), mode)
            pending.write(content)
            pending.flush()
            os.fsync(pending.fileno())
            os.replace(name, path)
        finally:
            if os.path.exists(name):
                os.unlink(name)


def configure(data, tool, script, python):
    if not isinstance(data, dict):
        raise ValueError(f'{tool} settings must contain a mapping')
    hooks = data.setdefault('hooks', {})
    if not isinstance(hooks, dict):
        raise ValueError(f'{tool} hooks must contain a mapping')
    # Detach YAML aliases before changing the selected hook subtree.
    hooks = data['hooks'] = copy.deepcopy(hooks)
    phases = [('on_session_start', 'recall'), ('on_session_end', 'flush')] if tool == 'hermes' else [('SessionStart', 'recall'), ('Stop', 'capture')]
    for event, phase in phases:
        entries = hooks.setdefault(event, [])
        if not isinstance(entries, list):
            raise ValueError(f'{tool} {event} hooks must contain a list')
        entries = hooks[event] = copy.deepcopy(entries)
        suffix = [str(script), '--tool', tool, '--phase', phase]
        command = shlex.join([python, *suffix])
        found = False
        for group in entries:
            if not isinstance(group, dict):
                raise ValueError(f'{tool} {event} contains an invalid hook')
            handlers = [group] if tool == 'hermes' else group.get('hooks')
            if not isinstance(handlers, list) or any(not isinstance(h, dict) for h in handlers):
                raise ValueError(f'{tool} {event} contains an invalid handler list')
            for handler in handlers:
                try:
                    parts = shlex.split(handler.get('command', ''))
                except (ValueError, TypeError):
                    continue
                if (len(parts) == 5 or (len(parts) == 6 and (parts[0] == python or re.fullmatch(r'python(?:3(?:\.\d+)?)?', Path(parts[0]).name)))) and parts[-4:] == suffix[-4:] and Path(parts[-5]).expanduser().resolve() == script.resolve():
                    handler['command'] = command
                    found = True
        if not found:
            handler = {'command': command, 'timeout': 15}
            if tool == 'hermes':
                entries.append(handler)
            else:
                handler['type'] = 'command'
                group = {'hooks': [handler]}
                if tool == 'claude' and event == 'SessionStart':
                    group['matcher'] = 'startup|resume|clear|compact'
                entries.append(group)


def install(home, tools, hook_source, gateway_url=None):
    if sys.version_info < (3, 10):
        raise ValueError('Memory hooks require Python 3.10 or newer')
    home = Path(home).expanduser().resolve()
    script = home / '.local/bin/gah-memory-hook'
    settings = home / '.config/gah/memory-hooks.json'
    if not tools or any(tool not in ('claude', 'codex', 'hermes') for tool in tools):
        raise ValueError('Select claude, codex, or hermes with --tool')
    tools = list(dict.fromkeys(tools))
    yaml = None
    if 'hermes' in tools:
        try:
            from ruamel.yaml import YAML
        except ImportError:
            raise ValueError('Hermes setup needs its ruamel.yaml library. Install Hermes or pass --python pointing to its venv/bin/python.') from None
        yaml = YAML()  # Round-trip mode preserves comments, quoting, and unrelated configuration.
        yaml.preserve_quotes = True
    if gateway_url is not None:
        url = urlsplit(gateway_url)
        if url.scheme not in ('http', 'https') or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ValueError('Use an HTTP(S) gateway URL without credentials, query, or fragment')
        url.port  # Reject invalid ports before touching any file.
        gateway_url = gateway_url.rstrip('/')
    settings.parent.mkdir(parents=True, exist_ok=True)
    lock_path = settings.with_suffix('.lock')
    with open(lock_path, 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        changes = []
        for tool in tools:
            path = home / { 'claude': '.claude/settings.json', 'codex': '.codex/hooks.json', 'hermes': '.hermes/config.yaml' }[tool]
            before = read_file(path)
            data = (yaml.load(before.decode()) if tool == 'hermes' else json.loads(before, object_pairs_hook=unique_object)) if before else {}
            if data is None and tool == 'hermes':
                data = {}
            configure(data, tool, script, sys.executable)
            if tool == 'hermes':
                output = io.StringIO()
                yaml.dump(data, output)
                after = output.getvalue().encode()
            else:
                after = (json.dumps(data, indent=2, ensure_ascii=False) + '\n').encode()
            changes.append((path, before, after, 0o600))
        # Parse gateway settings even without a URL update, so broken setup fails before writes.
        before = read_file(settings)
        gateway = json.loads(before, object_pairs_hook=unique_object) if before else {}
        if not isinstance(gateway, dict):
            raise ValueError('Memory-hook settings must contain an object')
        if gateway_url is not None:
            gateway['gateway_url'] = gateway_url
            changes.append((settings, before, (json.dumps(gateway, indent=2) + '\n').encode(), 0o600))
        changes.append((script, read_file(script), hook_source.encode(), 0o700))
        changes = [change for change in changes if change[1] != change[2]]
        # Preflight everything first; retain private backups before replacing any file.
        for path, before, _, _ in changes:
            if before is not None:
                with tempfile.NamedTemporaryFile(prefix=path.name + '.gah-backup-', dir=path.parent, delete=False) as backup:
                    backup.write(before)
                    backup.flush()
                    os.fsync(backup.fileno())
                print(f'Backup: {backup.name}')
        written = []
        try:
            for path, before, after, mode in changes:
                if read_file(path) != before:
                    raise ValueError(f'File changed during setup; retry: {path}')
                previous_mode = path.stat().st_mode & 0o777 if before is not None else mode
                atomic_write(path, after, mode)
                written.append((path, before, after, previous_mode))
        except Exception:
            for path, before, after, mode in reversed(written):
                if read_file(path) == after:
                    if before is None:
                        path.unlink()
                    else:
                        atomic_write(path, before, mode)
            raise
    print(f'Memory hooks configured for {", ".join(tools)} ({len(changes)} files changed).')
    if not (gateway.get('gateway_url') or os.environ.get('TDAI_GATEWAY_URL') or (home / '.config/gah/tdai-gateway.env').exists()):
        print('Gateway not configured: hooks remain inactive. Re-run with --gateway-url after gateway setup.')
    print('Setup does not contact the gateway. Unavailable gateways never block agent sessions.')
    if 'codex' in tools:
        print('In Codex, use /hooks to review and trust the new hooks; existing trust policy is unchanged.')
    if 'hermes' in tools:
        print('In Hermes, review the new hooks interactively before headless use; hooks_auto_accept is unchanged.')


if __name__ == '__main__':
    try:
        install(**json.load(sys.stdin))
    except Exception as error:
        print(f'Memory-hook setup failed: {error}', file=sys.stderr)
        sys.exit(1)
