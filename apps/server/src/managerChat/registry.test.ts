import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { resolveInstanceAdapter } from './registry.js';

test('named adapter cache enforces backend ownership and changes after credential replacement', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-instance-cache-'));
  const cli = join(dir, 'gah');
  const state = join(dir, 'runtime.json');
  const previous = process.env.GAH_BINARY;
  const runtime = { backend_instance: 'hermes-work', runner_kind: 'hermes', logical_backend: 'hermes',
    executable: '/tools/hermes', state_root: '/isolated/work', account_label: 'Work',
    credential_id: 'work-key', credential_provider: 'nous', credential_revision: 'generation-1' };
  writeFileSync(cli, `#!/bin/sh\ncat '${state}'\n`, { mode: 0o755 });
  try {
    process.env.GAH_BINARY = cli;
    writeFileSync(state, JSON.stringify(runtime));
    const first = await resolveInstanceAdapter('credential-test', 'hermes', 'hermes-work');
    assert.equal(await resolveInstanceAdapter('credential-test', 'hermes', 'hermes-work'), first);
    await assert.rejects(resolveInstanceAdapter('credential-test', 'opencode', 'hermes-work'), /does not serve opencode/);
    writeFileSync(state, JSON.stringify({ ...runtime, credential_revision: 'generation-2' }));
    assert.notEqual(await resolveInstanceAdapter('credential-test', 'hermes', 'hermes-work'), first);
  } finally {
    if (previous === undefined) delete process.env.GAH_BINARY; else process.env.GAH_BINARY = previous;
    rmSync(dir, { recursive: true, force: true });
  }
});
