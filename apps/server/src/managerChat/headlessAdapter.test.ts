import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { agyBackendSpec, createHeadlessBackend, type HeadlessBackendSpec } from './headlessAdapter.js';

/** A fake one-shot CLI: echoes its cwd marker file's content so the test
 * proves the turn ran in the session cwd, and echoes the prompt tail. */
function fakeCli(dir: string): string {
  const path = join(dir, 'fake-backend');
  writeFileSync(path, `#!/bin/sh
if [ -f ./MARKER ]; then echo "cwd-marker: $(cat ./MARKER)"; fi
echo "full-prompt: $1"
`, { mode: 0o755 });
  return path;
}

test('a headless turn runs in the conversation cwd and replays history into the prompt', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-'));
  const workdir = mkdtempSync(join(tmpdir(), 'gah-headless-wt-'));
  try {
    const cli = fakeCli(dir);
    const spec: HeadlessBackendSpec = {
      id: 'fake',
      displayName: 'Fake',
      turnArgs: (prompt) => [cli, prompt]
    };
    const backend = createHeadlessBackend(spec);

    writeFileSync(join(workdir, 'MARKER'), 'session-worktree');

    const history = [
      { role: 'user' as const, text: 'earlier question', timestamp: 1 },
      { role: 'assistant' as const, text: 'earlier answer', timestamp: 2 }
    ];
    const result = await backend.runTurn('p#s1', {
      prompt: 'new question',
      history,
      onChunk: () => {},
      onToolResult: () => {},
      cwd: workdir
    });

    assert.match(result.reply, /cwd-marker: session-worktree/, 'ran in the session cwd');
    assert.match(result.reply, /new question/, 'the new prompt is present');
    assert.match(result.reply, /earlier question/, 'prior history was replayed');
    assert.match(result.reply, /user: new question/, 'replay format matches resumePrompt');
  } finally {
    rmSync(dir, { recursive: true, force: true });
    rmSync(workdir, { recursive: true, force: true });
  }
});

test('a headless backend surfaces CLI failure text and rejects cleanly', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-fail-'));
  try {
    const cli = join(dir, 'failing');
    writeFileSync(cli, '#!/bin/sh\necho "boom: bad credentials" >&2\nexit 3\n', { mode: 0o755 });
    const backend = createHeadlessBackend({
      id: 'fake',
      displayName: 'Fake',
      turnArgs: (prompt) => [cli, prompt]
    });
    await assert.rejects(
      backend.runTurn('p#s2', {
        prompt: 'hi',
        history: [],
        onChunk: () => {},
        onToolResult: () => {}
      }),
      /boom: bad credentials/
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('headless backends advertise no config options and refuse config changes', async () => {
  const backend = createHeadlessBackend({
    id: 'fake',
    displayName: 'Fake',
    turnArgs: (prompt) => ['true', prompt]
  });
  assert.deepEqual(await backend.listCommands('p'), []);
  const models = await backend.listModels('p');
  assert.equal(models.models.length, 0);
  assert.equal(models.reasoningEfforts.length, 0);
  await assert.rejects(backend.setModel('p', 'x'), /headless mode/);
  await assert.rejects(backend.setReasoningEffort('p', 'high'), /headless mode/);
});

test('AGY receives the prompt as the value of --print before other flags', () => {
  assert.deepEqual(agyBackendSpec().turnArgs('three-turn remembered fact'), [
    'agy',
    '--print',
    'three-turn remembered fact',
    '--output-format',
    'text'
  ]);
});
