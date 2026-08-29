import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execPath } from 'node:process';
import type { ChatTranscriptTurn } from '@git-agent-harness/contracts';
import { agyBackendSpec, createHeadlessBackend, vibeBackendSpec, type HeadlessBackendSpec } from './headlessAdapter.js';

/** A fake one-shot CLI: echoes its cwd marker file's content so the test
 * proves the turn ran in the session cwd, and echoes the prompt tail. The
 * prompt arrives over stdin (issue #1009), never argv. */
function fakeCli(dir: string): string {
  const path = join(dir, 'fake-backend');
  writeFileSync(path, `#!/bin/sh
if [ -f ./MARKER ]; then echo "cwd-marker: $(cat ./MARKER)"; fi
echo "full-prompt: $(cat)"
`, { mode: 0o755 });
  return path;
}

/** History big enough that, replayed into one prompt, it would blow past a
 * conservative OS ARG_MAX (a few hundred KB) if it ever reached argv. */
function oversizedHistory(canary: string): ChatTranscriptTurn[] {
  const filler = 'x'.repeat(2000);
  const turns: ChatTranscriptTurn[] = [];
  for (let i = 0; i < 2000; i++) {
    turns.push({ role: 'user', text: `${filler}-${i}`, timestamp: i });
  }
  turns.push({ role: 'assistant', text: canary, timestamp: turns.length });
  return turns; // ~4MB of history, well past any conservative ARG_MAX
}

test('a headless turn runs in the conversation cwd and replays history into the prompt', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-'));
  const workdir = mkdtempSync(join(tmpdir(), 'gah-headless-wt-'));
  try {
    const cli = fakeCli(dir);
    const spec: HeadlessBackendSpec = {
      id: 'fake',
      displayName: 'Fake',
      turnArgs: () => [cli],
      encodeStdin: (prompt) => prompt
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
    writeFileSync(cli, '#!/bin/sh\ncat >/dev/null\necho "boom: bad credentials" >&2\nexit 3\n', { mode: 0o755 });
    const backend = createHeadlessBackend({
      id: 'fake',
      displayName: 'Fake',
      turnArgs: () => [cli],
      encodeStdin: (prompt) => prompt
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

test('a successful exit with empty trimmed stdout surfaces as an explicit error, not a silent empty reply', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-empty-'));
  try {
    const cli = join(dir, 'empty-output');
    writeFileSync(cli, '#!/bin/sh\ncat >/dev/null\necho "   "\nexit 0\n', { mode: 0o755 });
    const backend = createHeadlessBackend({
      id: 'fake',
      displayName: 'Fake',
      turnArgs: () => [cli],
      encodeStdin: (prompt) => prompt
    });
    await assert.rejects(
      backend.runTurn('p#s3', {
        prompt: 'hi',
        history: [],
        onChunk: () => {},
        onToolResult: () => {}
      }),
      /no output/
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cancelTurn kills the in-flight process and the turn rejects, leaving the session recoverable', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-cancel-'));
  try {
    // A single Node child (no shell, no grandchildren) so SIGTERM's default
    // disposition — immediate termination — is deterministic and doesn't
    // depend on shell exec-optimization or orphaned descendants holding
    // stdio pipes open.
    const backend = createHeadlessBackend({
      id: 'fake',
      displayName: 'Fake',
      turnArgs: () => [execPath, '-e', 'setTimeout(() => {}, 30000)'],
      encodeStdin: (prompt) => prompt
    });

    const turnPromise = backend.runTurn('p#cancel', {
      prompt: 'hi',
      history: [],
      onChunk: () => {},
      onToolResult: () => {}
    });
    await new Promise((resolve) => setTimeout(resolve, 200)); // let the child spawn
    await backend.cancelTurn('p#cancel');
    await assert.rejects(turnPromise);

    // The session is recoverable: a fresh turn on the same key still runs.
    const cli2 = fakeCli(dir);
    const backend2 = createHeadlessBackend({
      id: 'fake',
      displayName: 'Fake',
      turnArgs: () => [cli2],
      encodeStdin: (prompt) => prompt
    });
    const result = await backend2.runTurn('p#cancel', { prompt: 'again', history: [], onChunk: () => {}, onToolResult: () => {} });
    assert.match(result.reply, /again/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('headless backends advertise no config options and refuse config changes', async () => {
  const backend = createHeadlessBackend({
    id: 'fake',
    displayName: 'Fake',
    turnArgs: () => ['true'],
    encodeStdin: (prompt) => prompt
  });
  assert.deepEqual(await backend.listCommands('p'), []);
  const models = await backend.listModels('p');
  assert.equal(models.models.length, 0);
  assert.equal(models.reasoningEfforts.length, 0);
  await assert.rejects(backend.setModel('p', 'x'), /headless mode/);
  await assert.rejects(backend.setReasoningEffort('p', 'high'), /headless mode/);
});

test('AGY argv is fixed and content-free; the prompt travels over stdin as stream-json', () => {
  const spec = agyBackendSpec();
  assert.deepEqual(spec.turnArgs(), [
    'agy',
    '--input-format',
    'stream-json',
    '--output-format',
    'stream-json',
    '--dangerously-skip-permissions',
    '--sandbox'
  ]);
  assert.equal(
    spec.encodeStdin('three-turn remembered fact'),
    `${JSON.stringify({ event: 'user', message: { content: [{ type: 'text', text: 'three-turn remembered fact' }] } })}\n`
  );
});

test('Vibe argv is fixed and content-free regardless of prompt; the prompt travels over stdin, never sys.argv from exec', () => {
  const spec = vibeBackendSpec({ resolveInterpreter: () => '/fake/python3' });
  const args = spec.turnArgs();
  assert.deepEqual(args.slice(0, 2), ['/fake/python3', '-c']);
  assert.equal(args.length, 3, 'interpreter, -c, and one fixed bridge script — nothing else');
  assert.match(args[2], /sys\.argv/, 'the bridge sets sys.argv in-process rather than relying on exec argv');
  assert.equal(spec.encodeStdin('anything, arbitrarily large'), 'anything, arbitrarily large');
});

test('an oversized accumulated AGY prompt reaches the backend via stdin while argv stays a handful of fixed bytes', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-agy-oversized-'));
  const originalPath = process.env.PATH;
  try {
    const canary = 'CANARY_AGY_OVERSIZED_1009';
    const argvRecordPath = join(dir, 'argv.txt');
    const stdinRecordPath = join(dir, 'stdin.txt');
    const fakeAgy = join(dir, 'agy');
    writeFileSync(fakeAgy, `#!/bin/sh
printf '%s\\n' "$@" > "${argvRecordPath}"
cat > "${stdinRecordPath}"
echo '{"event":"result","result":{"status":"SUCCESS","response":"agy-fake-reply"}}'
`, { mode: 0o755 });
    process.env.PATH = `${dir}:${originalPath}`;

    const backend = createHeadlessBackend(agyBackendSpec());
    const result = await backend.runTurn('p#agy-oversized', {
      prompt: 'final question',
      history: oversizedHistory(canary),
      onChunk: () => {},
      onToolResult: () => {}
    });

    assert.equal(result.reply, 'agy-fake-reply');
    const capturedArgv = readFileSync(argvRecordPath, 'utf8');
    const capturedStdin = readFileSync(stdinRecordPath, 'utf8');
    assert.ok(!capturedArgv.includes(canary), 'the OS-level argv must never carry conversation content');
    assert.ok(capturedArgv.length < 1000, `argv stayed bounded (${capturedArgv.length} bytes) regardless of ~4MB of history`);
    assert.ok(capturedStdin.includes(canary), 'the prompt reached the backend over stdin');
  } finally {
    process.env.PATH = originalPath;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('an oversized accumulated Vibe prompt reaches the backend via stdin while argv stays a handful of fixed bytes', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-oversized-'));
  try {
    const canary = 'CANARY_VIBE_OVERSIZED_1009';
    const argvRecordPath = join(dir, 'argv.txt');
    const stdinRecordPath = join(dir, 'stdin.txt');
    const fakeInterpreter = join(dir, 'fake-python3');
    writeFileSync(fakeInterpreter, `#!/bin/sh
printf '%s\\n' "$@" > "${argvRecordPath}"
cat > "${stdinRecordPath}"
echo 'vibe-fake-reply'
`, { mode: 0o755 });

    const spec = vibeBackendSpec({ resolveInterpreter: () => fakeInterpreter });
    const backend = createHeadlessBackend(spec);
    const result = await backend.runTurn('p#vibe-oversized', {
      prompt: 'final question',
      history: oversizedHistory(canary),
      onChunk: () => {},
      onToolResult: () => {}
    });

    assert.equal(result.reply, 'vibe-fake-reply');
    const capturedArgv = readFileSync(argvRecordPath, 'utf8');
    const capturedStdin = readFileSync(stdinRecordPath, 'utf8');
    assert.ok(!capturedArgv.includes(canary), 'the OS-level argv must never carry conversation content');
    assert.ok(capturedArgv.length < 1000, `argv stayed bounded (${capturedArgv.length} bytes) regardless of ~4MB of history`);
    assert.ok(capturedStdin.includes(canary), 'the prompt reached the backend over stdin, where the bridge sets sys.argv in-process');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
