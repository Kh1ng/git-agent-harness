import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { resolveWebRoot } from './webRoot.js';

// Issue #1327: a fresh Linux install had no process serving the dashboard,
// because the server served static files only when GAH_WEB_ROOT was set.
test('the server serves the checkout build when no web root is configured', () => {
  const built = mkdtempSync(join(tmpdir(), 'gah-built-web-'));
  try {
    assert.equal(resolveWebRoot(undefined, built), null, 'nothing to serve before the app is built');
    writeFileSync(join(built, 'index.html'), '<!doctype html>', 'utf8');
    assert.equal(resolveWebRoot(undefined, built), built);
  } finally {
    rmSync(built, { recursive: true, force: true });
  }
});

test('a configured web root wins, and an empty one turns serving off', () => {
  const built = mkdtempSync(join(tmpdir(), 'gah-built-web-'));
  try {
    writeFileSync(join(built, 'index.html'), '<!doctype html>', 'utf8');
    assert.equal(resolveWebRoot('/srv/dashboard', built), resolve('/srv/dashboard'));
    // A host whose own web server serves the dashboard opts out explicitly.
    assert.equal(resolveWebRoot('', built), null);
    assert.equal(resolveWebRoot('  ', built), null);
  } finally {
    rmSync(built, { recursive: true, force: true });
  }
});
