#!/usr/bin/env bash
# Run the read-only native navigation test on an already-paired physical iPhone.
set -euo pipefail
: "${GAH_IOS_DEVICE:?Set the physical iPhone UDID}"
: "${GAH_IOS_TEAM:?Set the Apple development team}"
: "${GAH_IOS_CENTRAL_URL:?Set the HTTPS central origin}"
python3 - <<'PY_ORIGIN'
import os, urllib.parse
try:
    uri = urllib.parse.urlsplit(os.environ['GAH_IOS_CENTRAL_URL'])
    valid = uri.scheme in ('http', 'https') and uri.hostname and not uri.username and not uri.password
    valid = valid and not uri.query and not uri.fragment and uri.path in ('', '/')
    valid = valid and (uri.port is None or 0 < uri.port < 65536)
except ValueError:
    valid = False
if not valid:
    raise SystemExit('GAH_IOS_CENTRAL_URL must be an HTTP(S) origin without credentials or a path')
PY_ORIGIN
root=$(cd "$(dirname "$0")/.." && pwd)
output=$(mktemp -d "${TMPDIR:-/tmp}/gah-ios-central.XXXXXX")
echo "Evidence directory: $output"
xcodebuild -project "$root/apps/ios/GAH.xcodeproj" -scheme GAH \
  -configuration Debug -destination "platform=iOS,id=$GAH_IOS_DEVICE" \
  -derivedDataPath "$output/derived" "DEVELOPMENT_TEAM=$GAH_IOS_TEAM" \
  -allowProvisioningUpdates build-for-testing > "$output/build.log" 2>&1
# An xctestrun environment belongs to the test runner, rather than the build
# shell. Only the public origin and optional expected node enter this file.
python3 - "$output/derived/Build/Products" <<'PY'
import os, pathlib, plistlib, sys
files = list(pathlib.Path(sys.argv[1]).glob('*.xctestrun'))
if len(files) != 1:
    raise SystemExit('Expected one xctestrun file')
path = files[0]
with path.open('rb') as f:
    data = plistlib.load(f)
settings = {'GAH_IOS_LIVE_CENTRAL_URL': os.environ['GAH_IOS_CENTRAL_URL']}
if os.environ.get('GAH_IOS_EXPECTED_NODE'):
    settings['GAH_IOS_EXPECTED_NODE'] = os.environ['GAH_IOS_EXPECTED_NODE']
if data.get('TestConfigurations'):
    targets = [target for config in data['TestConfigurations'] for target in config['TestTargets']]
else:
    targets = [value for value in data.values() if isinstance(value, dict) and value.get('IsUITestBundle')]
if not targets:
    raise SystemExit('No UI test target in xctestrun')
for target in targets:
    target.setdefault('EnvironmentVariables', {}).update(settings)
with path.open('wb') as f:
    plistlib.dump(data, f)
PY
runfiles=("$output"/derived/Build/Products/*.xctestrun)
if xcodebuild test-without-building -xctestrun "${runfiles[0]}" \
  -destination "platform=iOS,id=$GAH_IOS_DEVICE" \
  -only-testing:GAHTests/ControllerTests/testPhysicalCentralNavigationAndBackgroundRecovery \
  -resultBundlePath "$output/central.xcresult" > "$output/test.log" 2>&1; then
  status=0
else
  status=$?
fi
if [ -d "$output/central.xcresult" ]; then
  if ! xcrun xcresulttool export attachments --path "$output/central.xcresult" --output-path "$output/screenshots"; then
    echo "Screenshot export failed; see $output/central.xcresult and $output/test.log." >&2
  fi
else
  echo "No result bundle was produced; see $output/test.log." >&2
fi
echo "Physical central navigation exit status: $status. Screenshots: $output/screenshots"
exit "$status"
