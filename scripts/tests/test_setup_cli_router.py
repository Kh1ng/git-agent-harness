import importlib.util
import io
from unittest.mock import patch
import json
from pathlib import Path
import tempfile
import http.server
import subprocess
import sys
import threading
import unittest

spec = importlib.util.spec_from_file_location("router", Path(__file__).parents[1] / "setup-cli-router.py")
router = importlib.util.module_from_spec(spec)
spec.loader.exec_module(router)


class RouterSetupTests(unittest.TestCase):
    def test_reinstall_preserves_keys_policy_and_other_candidates(self):
        with tempfile.TemporaryDirectory(prefix="gah router '") as temporary:
            root = Path(temporary)
            settings = router.configure(root, 8317)
            config = root / "router.yaml"
            config.write_text(config.read_text().replace('"round-robin"', '"fill-first"'))
            self.assertEqual(router.configure(root, 8317), settings)
            self.assertIn('"fill-first"', config.read_text())
            self.assertEqual((root / "cli-router.json").stat().st_mode & 0o777, 0o600)
            wrapper = router.configure_opencode(root, settings, "/bin/echo", ["claude-sonnet-4-6"])
            self.assertNotIn(settings["apiKey"], wrapper.read_text())
            self.assertNotIn(settings["apiKey"], (root / "router-opencode.json").read_text())
            original = '[profiles.gah]\nrepo="Kh1ng/git-agent-harness"\n[profiles.gah.routing]\nimprove_backend="agy"\n'
            gah = root / "config.toml"
            gah.write_text(original)
            router.register_instance(gah, "gah", root, wrapper)
            stored = gah.read_text()
            router.register_instance(gah, "gah", root, wrapper)
            self.assertEqual(gah.read_text(), stored)
            self.assertTrue(stored.startswith(original))
            self.assertEqual(router.tomllib.loads(stored)["profiles"]["gah"]["routing"]["improve_backend"], "agy")

    def test_corrupt_settings_are_not_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "cli-router.json"
            for text in ["{", "[]", '{"url":"http://127.0.0.1:8317"}']:
                path.write_text(text)
                with self.assertRaises(ValueError):
                    router.configure(root, 8317)
                self.assertEqual(path.read_text(), text)

    def test_agy_import_verifies_client_and_preserves_original_login(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            home = root / "agy"
            token_path = home / ".gemini/antigravity-cli/antigravity-oauth-token"
            token_path.parent.mkdir(parents=True)
            original = '{"token":{"refresh_token":"original-refresh"}}'
            token_path.write_text(original)
            responses = [{"access_token": "fresh-access", "expires_in": 3600}, {"email": "test@example.com"}, {"cloudaicompanionProject": {"id": "project-one"}}]
            with patch.object(router.urllib.request, "urlopen", side_effect=[io.BytesIO(b'ClientID = "test-client"\nClientSecret = "test-secret"'), *[io.BytesIO(json.dumps(r).encode()) for r in responses]]) as request:
                router.import_agy_account(root, home, "agy-1")
                self.assertIn(b"grant_type=refresh_token", request.call_args_list[1].args[0].data)
            imported = root / "router-auth/antigravity-agy-1.json"
            self.assertEqual(token_path.read_text(), original)
            self.assertEqual(json.loads(imported.read_text())["project_id"], "project-one")
            self.assertEqual(imported.stat().st_mode & 0o777, 0o600)
            with patch.object(router.urllib.request, "urlopen") as request:
                router.import_agy_account(root, home, "agy-1")
                request.assert_not_called()

    def test_wrapper_discovers_models_and_forwards_arguments_without_sharing(self):
        class Models(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.server.authorization = self.headers.get("Authorization")
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b'{"data":[{"id":"model-live"}]}')

            def log_message(self, *_args):
                pass

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            server = http.server.HTTPServer(("127.0.0.1", 0), Models)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                settings = router.configure(root, server.server_port)
                executable = root / "opencode"
                executable.write_text(f'#!{sys.executable}\nimport json,os,sys\nprint(json.dumps({{"argv":sys.argv[1:],"config":json.loads(os.environ["OPENCODE_CONFIG_CONTENT"])}}))\n')
                executable.chmod(0o700)
                wrapper = router.configure_opencode(root, settings, str(executable), [])
                result = json.loads(subprocess.check_output([str(wrapper), "run", "--model", "gah-router/model-live"]))
                self.assertEqual(result["argv"], ["run", "--model", "gah-router/model-live"])
                self.assertEqual(result["config"]["share"], "disabled")
                self.assertIn("model-live", result["config"]["provider"]["gah-router"]["models"])
                self.assertEqual(server.authorization, "Bearer " + settings["apiKey"])
            finally:
                server.shutdown()
                server.server_close()
                thread.join()


if __name__ == "__main__":
    unittest.main()
