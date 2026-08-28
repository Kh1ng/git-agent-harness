#!/usr/bin/env node
/**
 * Hash-cached local test runner.
 *
 * Dev-only accelerator: re-runs a configured test suite ONLY when anything
 * it depends on changed since the last *passing* run. Skips otherwise.
 *
 * Correctness guardrails:
 *   - The cache key is the content hash of every test file under the suite's
 *     source root plus every source/fixture file the suite plausibly depends
 *     on. Any edit to app server/web/contracts/shared code invalidates the
 *     server suite; any edit to web code should invalidate the web e2e.
 *   - Failures are never cached: a suite that failed last time always runs.
 *   - The cache lives under node_modules/.cache (machine-local, uncommitted).
 *   - CI never calls this; CI runs the exhaustive `npm run test:*` scripts.
 *
 * Usage:
 *   node scripts/test-cache.mjs server          # run or skip the server battery
 *   node scripts/test-cache.mjs web             # run or skip web playwright e2e
 *   node scripts/test-cache.mjs server --force  # ignore cache, always run
 *   node scripts/test-cache.mjs server --reset  # drop the cached verdict
 */
import { createHash } from 'node:crypto';
import { lstatSync, readdirSync, readFileSync, mkdirSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const cacheDir = join(repoRoot, 'node_modules', '.cache', 'gah-tests');

const CONFIG = {
  server: {
    cwd: join(repoRoot, 'apps', 'server'),
    command: ['npx', '--no-install', 'tsx', '--test', ...collect(join(repoRoot, 'apps', 'server', 'src'), /\.test\.ts$/)],
    roots: [
      join(repoRoot, 'apps', 'server'),
      join(repoRoot, 'apps', 'server', 'tests'),
      join(repoRoot, 'packages', 'contracts'),
      join(repoRoot, 'packages', 'shared')
    ]
  },
  web: {
    cwd: join(repoRoot, 'apps', 'web'),
    command: ['npx', 'playwright', 'test', '-c', join(repoRoot, 'apps', 'web', 'playwright.config.ts'), join(repoRoot, 'apps', 'web', 'tests', 'e2e')],
    roots: [
      join(repoRoot, 'apps', 'web', 'src'),
      join(repoRoot, 'apps', 'web', 'tests'),
      join(repoRoot, 'apps', 'server', 'src'),
      join(repoRoot, 'apps', 'server', 'tests', 'fixtures'),
      join(repoRoot, 'packages', 'contracts'),
      join(repoRoot, 'packages', 'shared')
    ]
  }
};

function collect(dir, re, out = []) {
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir)) {
    const p = join(dir, entry);
    if (lstatSync(p).isDirectory()) collect(p, re, out);
    else if (re.test(entry)) out.push(p);
  }
  return out;
}

function contentHash(files) {
  const hash = createHash('sha256');
  // Sort for determinism; node version included so a toolchain bump forces a rerun.
  hash.update(process.version);
  for (const file of [...files].sort()) {
    const rel = relative(repoRoot, file);
    const content = readFileSync(file);
    hash.update(rel).update('\0').update(content).update('\0');
  }
  return hash.digest('hex');
}

function suiteHash(profile) {
  const { roots } = CONFIG[profile];
  const files = [];
  for (const root of roots) {
    const actionable = lstatSync(root).isDirectory() ? collect(root, /\.(ts|tsx|js|json|html|css)$/) : [root];
    for (const f of actionable) {
      const rel = relative(repoRoot, f);
      if (rel.includes(`${sep}node_modules${sep}`)) continue;
      files.push(f);
    }
  }
  return contentHash(files);
}

const profile = process.argv[2];
const force = process.argv.includes('--force');
const reset = process.argv.includes('--reset');

if (!profile || !CONFIG[profile]) {
  console.error(`usage: node scripts/test-cache.mjs <${Object.keys(CONFIG).join('|')}> [--force|--reset]`);
  process.exit(2);
}

mkdirSync(cacheDir, { recursive: true });
const cacheFile = join(cacheDir, `${profile}.json`);
const hash = suiteHash(profile);

if (reset) {
  rmSync(cacheFile, { force: true });
  console.log(`[test-cache] cache cleared for '${profile}'`);
}

if (!force && existsSync(cacheFile)) {
  const cached = JSON.parse(readFileSync(cacheFile, 'utf8'));
  if (cached.hash === hash) {
    if (cached.exitCode === 0) {
      const seconds = (cached.ms / 1000).toFixed(1);
      console.log(`[test-cache] '${profile}' unchanged since a passing run (${seconds}s, ${cached.tests ?? '?'} tests, ${new Date(cached.when).toISOString()}) — SKIPPING. Use --force to re-run.`);
      process.exit(0);
    }
    console.log(`[test-cache] '${profile}' unchanged but last run FAILED (exit ${cached.exitCode}) — re-running.`);
  } else {
    console.log(`[test-cache] '${profile}' changed since cached verdict — running.`);
  }
} else if (!force) {
  console.log(`[test-cache] '${profile}' has no cached verdict — running.`);
}

const { cwd, command } = CONFIG[profile];
const started = Date.now();
console.log(`[test-cache] $ ${command.join(' ')}  (cwd: ${relative(repoRoot, cwd)})`);
const result = spawnSync(command[0], command.slice(1), { cwd, stdio: 'inherit', env: process.env });

const ms = Date.now() - started;
const passed = result.status === 0;
if (passed) {
  writeFileSync(cacheFile, JSON.stringify({ hash, exitCode: 0, ms, when: new Date().toISOString() }));
  console.log(`[test-cache] '${profile}' PASSED in ${(ms / 1000).toFixed(1)}s — verdict cached.`);
} else {
  // Never cache a failure: the next run re-executes the suite.
  rmSync(cacheFile, { force: true });
  console.log(`[test-cache] '${profile}' FAILED in ${(ms / 1000).toFixed(1)}s — not cached.`);
}
process.exit(result.status ?? 1);