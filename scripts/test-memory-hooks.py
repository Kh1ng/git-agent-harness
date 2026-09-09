"""Hermetic setup/rollback and hook checks; no real configs or gateway calls."""
import importlib.util
import json
from pathlib import Path
import shlex
import tempfile
import unittest
from unittest.mock import patch


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


setup = load('setup', 'install-memory-hooks.py')
hook = load('hook', 'gah-memory-hook.py')
SOURCE = Path(__file__).with_name('gah-memory-hook.py').read_text()


class MemoryHooks(unittest.TestCase):
    def test_merge_preserves_other_hooks_comments_consent_and_is_repeatable(self):
        with tempfile.TemporaryDirectory(prefix="gah hooks '$() ") as directory:
            root = Path(directory)
            for tool in ['claude', 'codex', 'hermes']:
                (root / f'.{tool}').mkdir()
            old_hook = {'type': 'command', 'command': 'petdex keep-me'}
            claude = root / '.claude/settings.json'
            claude.write_text(json.dumps({'permissions': {'allow': ['Read']}, 'hooks': {'SessionStart': [{'matcher': 'startup', 'hooks': [old_hook]}]}}))
            codex = root / '.codex/hooks.json'
            codex.write_text(json.dumps({'hooks': {'Stop': [{'hooks': [{'type':'command', 'command': shlex.join([str(root / '.local/bin/gah-memory-hook'), '--tool', 'codex', '--phase', 'capture'])}]}]}}))
            hermes = root / '.hermes/config.yaml'
            hermes.write_text('# Keep this note\nmodel: "existing-model"\nhooks_auto_accept: false\nhooks:\n  on_session_start:\n    - command: "petdex keep-me"\n')
            setup.install(root, ['claude', 'codex', 'hermes'], SOURCE, 'http://127.0.0.1:9')
            result = json.loads(claude.read_text())
            self.assertEqual(result['permissions'], {'allow':['Read']})
            self.assertEqual(result['hooks']['SessionStart'][0]['hooks'], [old_hook])
            command = result['hooks']['SessionStart'][1]['hooks'][0]['command']
            self.assertEqual(shlex.split(command)[1:], [str(root.resolve() / '.local/bin/gah-memory-hook'), '--tool', 'claude', '--phase', 'recall'])
            self.assertIn('# Keep this note', hermes.read_text())
            self.assertIn('model: "existing-model"', hermes.read_text())
            self.assertIn('hooks_auto_accept: false', hermes.read_text())
            self.assertIn('petdex keep-me', hermes.read_text())
            self.assertEqual(len(json.loads(codex.read_text())['hooks']['Stop']), 1)
            files = [claude, codex, hermes, root / '.local/bin/gah-memory-hook', root / '.config/gah/memory-hooks.json']
            before = {str(p):p.read_bytes() for p in root.rglob('*') if p.is_file()}
            setup.install(root, ['claude', 'codex', 'hermes'], SOURCE)
            self.assertEqual(before, {str(p):p.read_bytes() for p in root.rglob('*') if p.is_file()})
            self.assertEqual(files[3].stat().st_mode & 0o777, 0o700)
            self.assertTrue(all(p.stat().st_mode & 0o777 == 0o600 for p in root.rglob('*.gah-backup-*')))

    def test_unrelated_command_yaml_alias_and_symlinked_parent_are_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / '.local').mkdir()
            (root / 'bin').mkdir()
            (root / '.local/bin').symlink_to(root / 'bin', target_is_directory=True)
            (root / '.claude').mkdir()
            claude = root / '.claude/settings.json'
            diagnostic = shlex.join(['echo', str(root / '.local/bin/gah-memory-hook'), '--tool', 'claude', '--phase', 'recall'])
            claude.write_text(json.dumps({'hooks': {'SessionStart': [{'hooks': [{'type': 'command', 'command': diagnostic}]}]}}))
            (root / '.hermes').mkdir()
            hermes = root / '.hermes/config.yaml'
            hermes.write_text('other: &shared {}\nhooks: *shared\n')
            for _ in range(2):
                setup.install(root, ['claude', 'hermes'], SOURCE)
            groups = json.loads(claude.read_text())['hooks']['SessionStart']
            self.assertEqual(len(groups), 2)
            self.assertEqual(groups[0]['hooks'][0]['command'], diagnostic)
            from ruamel.yaml import YAML
            data = YAML().load(hermes.read_text())
            self.assertEqual(data['other'], {})
            self.assertEqual(len(data['hooks']['on_session_start']), 1)

    def test_invalid_input_and_write_failure_preserve_existing_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / '.claude').mkdir()
            (root / '.codex').mkdir()
            claude, codex = root / '.claude/settings.json', root / '.codex/hooks.json'
            claude.write_text('{"theme":"keep"}')
            codex.write_text('{broken')
            with self.assertRaises(ValueError):
                setup.install(root, ['claude', 'codex'], SOURCE)
            self.assertEqual(claude.read_text(), '{"theme":"keep"}')
            self.assertFalse((root / '.local/bin/gah-memory-hook').exists())
            codex.write_text('{}')
            write = setup.atomic_write
            calls = 0
            def fail_once(path, content, mode):
                nonlocal calls
                calls += 1
                if calls == 2:
                    raise OSError('fixture disk failure')
                return write(path, content, mode)
            with patch.object(setup, 'atomic_write', fail_once), self.assertRaises(OSError):
                setup.install(root, ['claude', 'codex'], SOURCE)
            self.assertEqual(claude.read_text(), '{"theme":"keep"}')
            self.assertEqual(codex.read_text(), '{}')
            self.assertFalse((root / '.local/bin/gah-memory-hook').exists())
            with self.assertRaises(ValueError):
                setup.install(root, ['claude'], SOURCE, 'https://user:secret@example.test')
            claude.unlink()
            claude.symlink_to(codex)
            with self.assertRaisesRegex(ValueError, 'symlink'):
                setup.install(root, ['claude'], SOURCE)
            self.assertEqual(codex.read_text(), '{}')

    def test_missing_gateway_is_inactive_and_unavailable_gateway_is_nonblocking(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'memory-hooks.json'
            with patch.object(hook, 'SETTINGS_FILE', config), patch.object(hook, 'TDAI_API_KEY_FILE', Path(directory) / 'missing.env'), patch.dict('os.environ', {}, clear=True), patch.object(hook.urllib.request.OpenerDirector, 'open') as request:
                self.assertIsNone(hook.gateway_post('/recall', {}))
                request.assert_not_called()
                config.write_text('{"gateway_url":"http://127.0.0.1:9"}')
                request.side_effect = hook.urllib.error.URLError('fixture unavailable')
                self.assertIsNone(hook.gateway_post('/recall', {}))
                self.assertEqual(request.call_count, 1)
                self.assertIsNone(hook.NoGatewayRedirect().redirect_request(None, None, 302, '', {}, 'https://other.test'))


if __name__ == '__main__':
    unittest.main()
