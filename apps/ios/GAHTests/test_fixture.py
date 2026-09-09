"""An idle WebKit connection must not block the simulator's control requests."""
from pathlib import Path
import socket
import subprocess
import sys
import time
import unittest
from urllib.request import urlopen


class FixtureTests(unittest.TestCase):
    def test_idle_connection_does_not_block_control_requests(self):
        fixture = subprocess.Popen([sys.executable, str(Path(__file__).with_name('fixture.py'))])
        try:
            deadline = time.monotonic() + 5
            while True:
                try:
                    with urlopen('http://127.0.0.1:18773/', timeout=0.2) as response:
                        self.assertEqual(response.status, 200)
                    break
                except OSError:
                    if fixture.poll() is not None or time.monotonic() >= deadline:
                        raise
                    time.sleep(0.01)
            with socket.create_connection(('127.0.0.1', 18773)):
                with urlopen('http://127.0.0.1:18773/arm-recovery', timeout=1) as response:
                    self.assertEqual(response.status, 200)
        finally:
            fixture.terminate()
            fixture.wait(timeout=5)


if __name__ == '__main__':
    unittest.main()
