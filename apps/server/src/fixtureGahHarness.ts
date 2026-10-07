// Issue #635 AC4: the fixture harness, split out of *.test.ts so it can be
// imported by other test suites -- this repo's own apps/server unit tests
// today, and the web e2e ticket (#636) later -- without pulling in
// node:test. Not itself a *.test.ts file, so it is never picked up as a
// test by `tsx --test src/*.test.ts`. Lives in src/, not tests/, only so
// `tsc`'s rootDir (scoped to src/ to keep dist/'s existing output layout
// stable) can typecheck it; the fixture binary + recorded responses it
// wraps still live under tests/fixtures/gah/ per the issue.
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { createServer } from './server.js';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

/** Absolute path to the fake `gah` binary -- point `GAH_BINARY` at this. */
export const FIXTURE_GAH_BINARY = resolve(__dirname, '../tests/fixtures/gah/gah');

export interface FixtureFailure {
  /** One of the subcommand names in tests/fixtures/gah/README.md. */
  command: string;
  message?: string;
  code?: number;
}

/**
 * Starts a real Express app + HTTP listener with `GAH_BINARY` pointed at
 * the fixture, restoring every mutated env var afterward. `fail`, when
 * set, makes exactly one fixture subcommand exit non-zero instead of
 * replaying its recorded response.
 */
export async function withFixtureServer(
  testFn: (baseUrl: string) => Promise<void>,
  fail?: FixtureFailure
): Promise<void> {
  resetCachedCoordinatorIdentity();
  const tmpIdentityDir = mkdtempSync(join(tmpdir(), 'gah-fixture-identity-'));
  const saved = {
    GAH_BINARY: process.env.GAH_BINARY,
    GAH_COORDINATOR_IDENTITY_PATH: process.env.GAH_COORDINATOR_IDENTITY_PATH,
    GAH_FIXTURE_FAIL: process.env.GAH_FIXTURE_FAIL,
    GAH_FIXTURE_FAIL_MESSAGE: process.env.GAH_FIXTURE_FAIL_MESSAGE,
    GAH_FIXTURE_FAIL_CODE: process.env.GAH_FIXTURE_FAIL_CODE
  };

  process.env.GAH_BINARY = FIXTURE_GAH_BINARY;
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(tmpIdentityDir, 'coordinator-identity.json');
  if (fail) {
    process.env.GAH_FIXTURE_FAIL = fail.command;
    if (fail.message !== undefined) process.env.GAH_FIXTURE_FAIL_MESSAGE = fail.message;
    if (fail.code !== undefined) process.env.GAH_FIXTURE_FAIL_CODE = String(fail.code);
  } else {
    delete process.env.GAH_FIXTURE_FAIL;
  }
  resetCachedCoordinatorIdentity();

  const app = createServer();
  const server = http.createServer(app);
  await new Promise<void>((resolvePromise) => server.listen(0, resolvePromise));
  const { port } = server.address() as AddressInfo;

  try {
    await testFn(`http://127.0.0.1:${port}`);
  } finally {
    await new Promise<void>((resolvePromise) => server.close(() => resolvePromise()));
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    resetCachedCoordinatorIdentity();
  }
}

let profileCounter = 0;
/** A distinct profile name avoids runStatus's 30s AsyncTtlCache serving one
 * test's result to another within the same process. */
export function uniqueFixtureProfile(): string {
  profileCounter += 1;
  return `fixture-${profileCounter}`;
}
