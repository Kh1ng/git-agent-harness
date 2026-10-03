"""Check evidence-export failures without contacting a phone or central."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class PhysicalTestExitStatus(unittest.TestCase):
    def test_preserves_test_status_when_evidence_is_missing_or_export_fails(self):
        helper = Path(__file__).with_name('test-ios-central.sh')
        for status, bundle in [(65, False), (65, True), (0, True)]:
            with self.subTest(status=status, bundle=bundle), tempfile.TemporaryDirectory() as stage:
                root = Path(stage)
                build = root / 'xcodebuild'
                build.write_text('''#!/usr/bin/env python3
import os, pathlib, plistlib, sys
args = sys.argv[1:]
if 'build-for-testing' in args:
    products = pathlib.Path(args[args.index('-derivedDataPath')+1], 'Build/Products')
    products.mkdir(parents=True)
    (products/'test.xctestrun').write_bytes(plistlib.dumps({'Tests': {'IsUITestBundle': True}}))
else:
    if os.environ['MAKE_BUNDLE'] == '1':
        pathlib.Path(args[args.index('-resultBundlePath')+1]).mkdir()
    sys.exit(int(os.environ['TEST_STATUS']))
''')
                export = root / 'xcrun'
                export.write_text('#!/bin/sh\nexit 74\n')
                build.chmod(0o755)
                export.chmod(0o755)
                env = dict(os.environ, PATH=f'{root}:' + os.environ['PATH'], TMPDIR=stage,
                           GAH_IOS_DEVICE='test-device', GAH_IOS_TEAM='test-team',
                           GAH_IOS_CENTRAL_URL='https://central.example.test',
                           TEST_STATUS=str(status), MAKE_BUNDLE=str(int(bundle)))
                result = subprocess.run(['bash', str(helper)], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
                self.assertIn('Screenshot export failed' if bundle else 'No result bundle', result.stderr)


if __name__ == '__main__':
    unittest.main()
