"""Contract tests for the smoke gate, not substitutes for an artifact run."""
import importlib.util
from pathlib import Path
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('smoke', Path(__file__).parents[1] / 'test-linux-appimage.py')
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class States:
    def __init__(self, values):
        self.values = values

    def contains(self, value):
        return value in self.values


class Node:
    def __init__(self, name, role='label', states=()):
        self.name, self.role, self.states = name, role, states

    def queryText(self):
        raise NotImplementedError

    def getRoleName(self):
        return self.role

    def getState(self):
        return States(self.states)


class Atspi:
    STATE_SHOWING = 'showing'
    STATE_ENABLED = 'enabled'
    STATE_SENSITIVE = 'sensitive'


class GateTests(unittest.TestCase):
    def nodes(self):
        return [Node(text) for text in smoke.REQUIRED_TEXT] + [
            Node('', 'document web', ['showing']),
            Node('Set up standalone', 'push button', ['enabled', 'sensitive']),
        ]

    def test_real_document_and_dynamic_fresh_install_result_required(self):
        result, _ = smoke.onboarding(self.nodes(), Atspi)
        self.assertTrue(result['passed'])
        for index in range(len(self.nodes())):
            nodes = self.nodes()
            del nodes[index]
            self.assertFalse(smoke.onboarding(nodes, Atspi)[0]['passed'])

    def test_native_menu_or_blank_document_cannot_pass(self):
        self.assertFalse(smoke.onboarding([Node('GAH', 'frame'), Node('Settings', 'menu item')], Atspi)[0]['passed'])
        self.assertFalse(smoke.onboarding([Node('', 'document web', ['showing'])], Atspi)[0]['passed'])

    def test_no_display_is_unsupported_and_retains_artifact_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / 'probe.AppImage'
            artifact.write_bytes(b'prerequisite probe; not a GUI artifact')
            output = root / 'proof'
            env = dict(os.environ)
            env.pop('DISPLAY', None)
            result = subprocess.run([
                sys.executable, str(Path(smoke.__file__)), str(artifact),
                '--environment', 'headless', '--evidence', str(output),
            ], env=env, capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 2)
            report = json.loads((output / 'result.json').read_text())
            self.assertEqual(report['status'], 'unsupported')
            self.assertFalse(report['passed'])
            self.assertEqual(report['sha256'], hashlib.sha256(artifact.read_bytes()).hexdigest())
            self.assertFalse((output / 'initial.png').exists())

    def test_hidden_document_or_disabled_action_cannot_pass(self):
        for index in (-1, -2):
            nodes = self.nodes()
            nodes[index].states = []
            self.assertFalse(smoke.onboarding(nodes, Atspi)[0]['passed'])


if __name__ == '__main__':
    unittest.main()
