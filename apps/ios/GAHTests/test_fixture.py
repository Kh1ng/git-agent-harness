"""An idle WebKit connection must not block the simulator's control requests."""
from pathlib import Path
import socket
import subprocess
import sys
import time
import unittest
from urllib.request import ProxyHandler, build_opener


class FixtureTests(unittest.TestCase):
    def test_loopback_startup_and_idle_connections(self):
        # Loopback fixture requests must not inherit the runner's outbound proxy.
        urlopen = build_opener(ProxyHandler({})).open
        fixture = subprocess.Popen([sys.executable, '-c',
            "import faulthandler, runpy, sys; from unittest.mock import patch; "
            "faulthandler.dump_traceback_later(20); "
            "patch('socket.getfqdn', side_effect=AssertionError('Loopback fixture must not resolve DNS')).start(); "
            "runpy.run_path(sys.argv[1], run_name='__main__')",
            str(Path(__file__).with_name('fixture.py'))], stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 30
            while True:
                try:
                    with urlopen('http://127.0.0.1:18773/', timeout=0.2) as response:
                        self.assertEqual(response.status, 200)
                    break
                except OSError:
                    if fixture.poll() is not None or time.monotonic() >= deadline:
                        try:
                            with socket.create_connection(('127.0.0.1', 18773), timeout=1):
                                listening = 'direct connection succeeded'
                        except OSError as error:
                            listening = str(error)
                        listeners = subprocess.run(['lsof', '-nP', '-iTCP:18773', '-sTCP:LISTEN'], capture_output=True, text=True).stdout
                        exit_code = fixture.poll()
                        fixture.terminate()
                        _, stderr = fixture.communicate(timeout=5)
                        self.fail(f'Fixture readiness failed: child exit={exit_code}; {listening}; listeners={listeners}; stderr={stderr}')
                    time.sleep(0.01)
            with socket.create_connection(('127.0.0.1', 18773)):
                with urlopen('http://127.0.0.1:18773/arm-recovery', timeout=1) as response:
                    self.assertEqual(response.status, 200)
        finally:
            fixture.terminate()
            fixture.communicate(timeout=5)


if __name__ == '__main__':
    unittest.main()
