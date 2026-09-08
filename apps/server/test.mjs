import { readdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

// Discover every server source test so adding a directory cannot silently
// remove its coverage from CI. Mock-control-plane tests have their own suite.
function testsIn(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? testsIn(path) : entry.name.endsWith('.test.ts') ? [path] : [];
  });
}

const root = dirname(fileURLToPath(import.meta.url));
const tests = testsIn(join(root, 'src')).sort();
if (tests.length === 0) throw new Error('No server tests found');
console.log(`Running ${tests.length} server test files`);
const result = spawnSync(process.execPath, ['--import', 'tsx', '--test', '--test-concurrency=2', ...process.argv.slice(2), ...tests], {
  cwd: root,
  stdio: 'inherit'
});
if (result.error) throw result.error;
process.exit(result.status ?? 1);
