import { isUsageLimitError } from './acpAdapter.js';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execPath } from 'node:process';
import type { ChatTranscriptTurn } from '@git-agent-harness/contracts';
import { agyBackendSpec, cursorBackendSpec, createHeadlessBackend, decodeVibeToolRequest, openhandsBackendSpec, parseOpenhandsReply, vibeBackendSpec, type HeadlessBackendSpec } from './headlessAdapter.js';

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
    let cli = [execPath, '-e', 'setTimeout(() => {}, 30000)'];
    const backend = createHeadlessBackend({
      id: 'fake',
      displayName: 'Fake',
      turnArgs: () => cli,
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

    // The same backend/session is recoverable: swap in a real command and
    // run a second turn on the same profile key, proving cancelTurn cleared
    // state.child rather than leaving the session wedged.
    cli = [fakeCli(dir)];
    const result = await backend.runTurn('p#cancel', { prompt: 'again', history: [], onChunk: () => {}, onToolResult: () => {} });
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

test('AGY advertises its live model catalog and native reasoning efforts', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-agy-models-'));
  const originalPath = process.env.PATH;
  try {
    writeFileSync(join(dir, 'agy'), `#!/bin/sh
cat >/dev/null
printf 'Fetching available models...\\ngemini-3.7-flash-high\\tGemini 3.7 Flash (High)\\nclaude-sonnet-4-6\\tClaude Sonnet 4.6 (Thinking)\\n'
`, { mode: 0o755 });
    process.env.PATH = `${dir}:${originalPath}`;

    const backend = createHeadlessBackend(agyBackendSpec());
    const summary = await backend.listModels('p');
    assert.deepEqual(summary.models, [
      { id: 'gemini-3.7-flash-high', name: 'Gemini 3.7 Flash (High)' },
      { id: 'claude-sonnet-4-6', name: 'Claude Sonnet 4.6 (Thinking)' }
    ]);
    assert.deepEqual(summary.reasoningEfforts, [
      { id: 'low', name: 'Low' },
      { id: 'medium', name: 'Medium' },
      { id: 'high', name: 'High' }
    ]);
    await backend.setModel('p', 'claude-sonnet-4-6');
    await backend.setReasoningEffort('p', 'high');
    const selected = await backend.listModels('p');
    assert.equal(selected.currentModelId, 'claude-sonnet-4-6');
    assert.equal(selected.currentReasoningEffortId, 'high');
    await assert.rejects(backend.setModel('p', 'missing'), /Unknown model/);
    await assert.rejects(backend.setReasoningEffort('p', 'ultra'), /Unknown reasoning effort/);
  } finally {
    process.env.PATH = originalPath;
    rmSync(dir, { recursive: true, force: true });
  }
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

test('OpenHands reads the replayed prompt from /dev/stdin and receives profile credentials only through env', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-openhands-'));
  const originalPath = process.env.PATH;
  try {
    const record = join(dir, 'record.json');
    writeFileSync(join(dir, 'openhands'), `#!/bin/sh
node -e 'const fs=require("fs"); fs.writeFileSync(process.argv[1], JSON.stringify({argv:process.argv.slice(2), stdin:fs.readFileSync(0,"utf8"), model:process.env.LLM_MODEL, key:process.env.LLM_API_KEY}))' "${record}" "$@"
echo '{"source":"agent","kind":"MessageEvent","llm_message":{"content":[{"type":"text","text":"openhands reply"}]}}'
`, { mode: 0o755 });
    process.env.PATH = `${dir}:${originalPath}`;
    const backend = createHeadlessBackend(openhandsBackendSpec({
      resolveEnv: async () => ({ LLM_MODEL: 'profile-model', LLM_API_KEY: 'profile-key', LLM_BASE_URL: 'http://profile' })
    }));
    const result = await backend.runTurn('repo#openhands', {
      prompt: 'new question',
      history: [{ role: 'user', text: 'old question', timestamp: 1 }],
      onChunk: () => {},
      onToolResult: () => {}
    });
    const captured = JSON.parse(readFileSync(record, 'utf8')) as { argv: string[]; stdin: string; model: string; key: string };
    assert.equal(result.reply, 'openhands reply');
    assert.deepEqual(captured.argv, openhandsBackendSpec().turnArgs().slice(1));
    assert.match(captured.stdin, /old question/);
    assert.match(captured.stdin, /new question/);
    assert.equal(captured.model, 'profile-model');
    assert.equal(captured.key, 'profile-key');
    assert.ok(!captured.argv.join(' ').includes('question'));
  } finally {
    process.env.PATH = originalPath;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('OpenHands JSONL parser accepts finish actions and rejects tool-only output', () => {
  assert.equal(parseOpenhandsReply({
    stdout: '{"source":"agent","tool_name":"finish","action":{"message":"finished"}}\n',
    stderr: '',
    exitCode: 0
  }), 'finished');
  assert.throws(() => parseOpenhandsReply({
    stdout: '{"source":"tool","content":"secret tool output"}\n',
    stderr: '',
    exitCode: 0
  }), /no assistant reply/);
});

test('Vibe has no model/effort flag, so a session pin is silently ignored rather than threaded through (#1032 reopened)', () => {
  const spec = vibeBackendSpec({ resolveInterpreter: () => '/fake/python3' });
  const unpinned = spec.turnArgs();
  const pinned = spec.turnArgs({ model: 'claude-sonnet-4.6-thinking', reasoningEffort: 'high' });
  assert.deepEqual(pinned, unpinned, 'Vibe argv is unaffected by a session pin -- Vibe stays unchanged');
});

test('#1032 reopened: a pinned session reaches fake AGY as --model and --effort alongside the existing permission/sandbox flags', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-agy-pinned-'));
  const originalPath = process.env.PATH;
  try {
    const argvRecordPath = join(dir, 'argv.txt');
    const fakeAgy = join(dir, 'agy');
    writeFileSync(fakeAgy, `#!/bin/sh
printf '%s\\n' "$@" > "${argvRecordPath}"
cat >/dev/null
echo '{"event":"result","result":{"status":"SUCCESS","response":"agy-fake-reply"}}'
`, { mode: 0o755 });
    process.env.PATH = `${dir}:${originalPath}`;

    const backend = createHeadlessBackend(agyBackendSpec());
    const result = await backend.runTurn('p#agy-pinned', {
      prompt: 'question',
      history: [],
      onChunk: () => {},
      onToolResult: () => {},
      model: 'claude-sonnet-4.6-thinking',
      reasoningEffort: 'high'
    });

    assert.equal(result.reply, 'agy-fake-reply');
    const capturedArgv = readFileSync(argvRecordPath, 'utf8').trim().split('\n');
    assert.deepEqual(capturedArgv, [
      '--input-format',
      'stream-json',
      '--output-format',
      'stream-json',
      '--dangerously-skip-permissions',
      '--sandbox',
      '--model',
      'claude-sonnet-4.6-thinking',
      '--effort',
      'high'
    ]);
  } finally {
    process.env.PATH = originalPath;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('#1032 reopened: an unpinned session emits neither --model nor --effort to fake AGY', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-agy-unpinned-'));
  const originalPath = process.env.PATH;
  try {
    const argvRecordPath = join(dir, 'argv.txt');
    const fakeAgy = join(dir, 'agy');
    writeFileSync(fakeAgy, `#!/bin/sh
printf '%s\\n' "$@" > "${argvRecordPath}"
cat >/dev/null
echo '{"event":"result","result":{"status":"SUCCESS","response":"agy-fake-reply"}}'
`, { mode: 0o755 });
    process.env.PATH = `${dir}:${originalPath}`;

    const backend = createHeadlessBackend(agyBackendSpec());
    await backend.runTurn('p#agy-unpinned', {
      prompt: 'question',
      history: [],
      onChunk: () => {},
      onToolResult: () => {}
    });

    const capturedArgv = readFileSync(argvRecordPath, 'utf8');
    assert.ok(!capturedArgv.includes('--model'), 'no model pin means no --model flag');
    assert.ok(!capturedArgv.includes('--effort'), 'no effort pin means no --effort flag');
  } finally {
    process.env.PATH = originalPath;
    rmSync(dir, { recursive: true, force: true });
  }
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

/** A fake vibe interpreter that leaks the observed #1041 wire shape on its
 * first invocation, then — once the continuation prompt carries the read
 * file's content — emits the review text. Each prompt is recorded to its
 * own file so tests assert exactly what each invocation received. */
function fakeVibeInterpreter(dir: string, targetMarker: string): string {
  const path = join(dir, 'fake-python3');
  const firstStdin = join(dir, 'stdin-1.txt');
  const secondStdin = join(dir, 'stdin-2.txt');
  writeFileSync(path, `#!/bin/sh
prompt=$(cat)
case "$prompt" in
  *${targetMarker}*)
    printf '%s' "$prompt" > "${secondStdin}"
    echo 'Review complete: the target file is intact.'
    ;;
  *)
    printf '%s' "$prompt" > "${firstStdin}"
    printf 'read_file죰{"file_path": "target.md"}'
    ;;
esac
`, { mode: 0o755 });
  return path;
}

test('decodeVibeToolRequest parses the observed U+C8F0 wire shape and rejects undecodable requests', () => {
  const observed = 'read_file죰{"file_path": "/path/to/apps/server/src/managerChat/memoryGatewayClient.ts"}';
  assert.deepEqual(decodeVibeToolRequest(observed), {
    name: 'read_file',
    args: { file_path: '/path/to/apps/server/src/managerChat/memoryGatewayClient.ts' },
    raw: observed
  });
  assert.equal(decodeVibeToolRequest('an ordinary assistant reply'), null);
  assert.equal(decodeVibeToolRequest(''), null);
  assert.equal(decodeVibeToolRequest('text before read_file죰{"file_path": "x"}'), null, 'only a bare tool request decodes');
  assert.equal(decodeVibeToolRequest('read_file죰{"file_path": "x"} trailing text'), null);
  assert.throws(() => decodeVibeToolRequest('read_file죰{"file_path": }'), /not valid JSON/);
  assert.equal(decodeVibeToolRequest('read_file죰{"file_path": '), null, 'truncated syntax without a closing brace is not a decodable request');
});

test('only vibe declares a tool-request decoder; agy is untouched', () => {
  assert.equal(vibeBackendSpec({ resolveInterpreter: () => '/fake/python3' }).decodeToolRequest?.('read_file죰{}')?.name, 'read_file');
  assert.equal(agyBackendSpec().decodeToolRequest, undefined);
});

test('a leaked Vibe tool request is decoded, permission-gated, executed, and the turn continues with the result replayed', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-tool-'));
  const workdir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-wt-'));
  try {
    writeFileSync(join(workdir, 'target.md'), 'VIBE-TARGET-CONTENT-1041');
    const interpreter = fakeVibeInterpreter(dir, 'VIBE-TARGET-CONTENT-1041');
    const backend = createHeadlessBackend(vibeBackendSpec({ resolveInterpreter: () => interpreter }));

    const toolCalls: { toolCallId: string; name: string | null; status: string; locations: string[]; summary: string | null }[] = [];
    const toolResults: { name: string; text: string }[] = [];
    const result = await backend.runTurn('p#vibe-tool', {
      prompt: 'review target.md',
      history: [],
      onChunk: () => {},
      onToolResult: (name, text) => toolResults.push({ name, text }),
      cwd: workdir,
      onToolCall: (tool) => toolCalls.push(tool),
      requestPermission: async (request) => {
        assert.deepEqual(request.locations, [join(workdir, 'target.md')]);
        return 'allow-once';
      }
    });

    // The review continues after the read: the final reply is the
    // continuation's output, never the raw tool syntax.
    assert.equal(result.reply, 'Review complete: the target file is intact.');
    assert.ok(!result.reply.includes('\u{C8F0}'));
    assert.deepEqual(toolCalls.map((tool) => tool.status), ['pending', 'completed']);
    assert.equal(toolCalls[0].name, 'read_file');
    assert.equal(toolCalls[0].toolCallId, toolCalls[1].toolCallId, 'one card per decoded request');
    assert.deepEqual(toolCalls[0].locations, [join(workdir, 'target.md')], 'locations resolve against the session cwd');
    assert.equal(toolCalls[1].summary, 'VIBE-TARGET-CONTENT-1041');
    assert.deepEqual(toolResults, [{ name: 'read_file', text: 'VIBE-TARGET-CONTENT-1041' }]);

    // The continuation prompt replays the exchange so the backend sees its
    // own request plus the tool result.
    const firstPrompt = readFileSync(join(dir, 'stdin-1.txt'), 'utf8');
    assert.ok(!firstPrompt.includes('read_file'), 'the first invocation only carries the replayed conversation');
    const secondPrompt = readFileSync(join(dir, 'stdin-2.txt'), 'utf8');
    assert.match(secondPrompt, /user: review target\.md/);
    assert.match(secondPrompt, /assistant: read_file죰\{"file_path": "target\.md"\}/);
    assert.match(secondPrompt, /tool: VIBE-TARGET-CONTENT-1041/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
    rmSync(workdir, { recursive: true, force: true });
  }
});

test('a declined vibe tool request fails the turn with an actionable error and never continues', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-decline-'));
  const workdir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-decline-wt-'));
  try {
    writeFileSync(join(workdir, 'target.md'), 'VIBE-TARGET-CONTENT-1041');
    const interpreter = fakeVibeInterpreter(dir, 'VIBE-TARGET-CONTENT-1041');
    const backend = createHeadlessBackend(vibeBackendSpec({ resolveInterpreter: () => interpreter }));

    const toolCalls: { status: string; summary: string | null }[] = [];
    await assert.rejects(
      backend.runTurn('p#vibe-decline', {
        prompt: 'review target.md',
        history: [],
        onChunk: () => {},
        onToolResult: () => {},
        cwd: workdir,
        onToolCall: (tool) => toolCalls.push({ status: tool.status, summary: tool.summary }),
        requestPermission: async () => 'reject-once'
      }),
      /declined \(reject-once\); the turn cannot continue/
    );
    assert.deepEqual(toolCalls.map((tool) => tool.status), ['pending', 'failed']);
    assert.match(toolCalls[1].summary ?? '', /declined/);
    assert.throws(() => readFileSync(join(dir, 'stdin-2.txt'), 'utf8'), /ENOENT/, 'no continuation invocation after a decline');
  } finally {
    rmSync(dir, { recursive: true, force: true });
    rmSync(workdir, { recursive: true, force: true });
  }
});

test('a decoded tool request with no permission UI attached fails closed instead of executing', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-noperm-'));
  const workdir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-noperm-wt-'));
  try {
    writeFileSync(join(workdir, 'target.md'), 'VIBE-TARGET-CONTENT-1041');
    const interpreter = fakeVibeInterpreter(dir, 'VIBE-TARGET-CONTENT-1041');
    const backend = createHeadlessBackend(vibeBackendSpec({ resolveInterpreter: () => interpreter }));
    await assert.rejects(
      backend.runTurn('p#vibe-noperm', {
        prompt: 'review target.md',
        history: [],
        onChunk: () => {},
        onToolResult: () => {},
        cwd: workdir
      }),
      /permission request was cancelled/
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
    rmSync(workdir, { recursive: true, force: true });
  }
});

test('undecodable or unservable vibe tool requests end the turn with an actionable error, never raw text', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-badtool-'));
  const workdir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-badtool-wt-'));
  try {
    const runWithReply = (reply: string) => {
      const interpreter = join(dir, `fake-python3-${reply.replace(/\W/g, '')}`);
      writeFileSync(interpreter, `#!/bin/sh
cat >/dev/null
printf '${reply}'
`, { mode: 0o755 });
      return createHeadlessBackend(vibeBackendSpec({ resolveInterpreter: () => interpreter })).runTurn('p#vibe-badtool', {
        prompt: 'go',
        history: [],
        onChunk: () => {},
        onToolResult: () => {},
        cwd: workdir,
        requestPermission: async () => 'allow-once'
      });
    };
    await assert.rejects(
      runWithReply('rm_rf죰{}'),
      /requested tool "rm_rf", which GAH cannot execute .*Executable tools: read_file/
    );
    await assert.rejects(
      runWithReply('read_file죰{"file_path": }'),
      /arguments are not valid JSON/
    );
    await assert.rejects(
      runWithReply('read_file죰{}'),
      /without a valid "file_path" argument/
    );
    await assert.rejects(
      runWithReply('read_file죰{"file_path": "../outside.txt"}'),
      /resolves outside this conversation's working directory/
    );
    await assert.rejects(
      runWithReply('read_file죰{"file_path": "missing.md"}'),
      /the read failed/
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
    rmSync(workdir, { recursive: true, force: true });
  }
});

test('a vibe turn stops before servicing more than the bounded number of tool requests', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-loop-'));
  const workdir = mkdtempSync(join(tmpdir(), 'gah-headless-vibe-loop-wt-'));
  try {
    writeFileSync(join(workdir, 'target.md'), 'VIBE-TARGET-CONTENT-1041');
    const interpreter = join(dir, 'fake-python3');
    writeFileSync(interpreter, `#!/bin/sh
cat >/dev/null
printf 'read_file죰{"file_path": "target.md"}'
`, { mode: 0o755 });
    const backend = createHeadlessBackend(vibeBackendSpec({ resolveInterpreter: () => interpreter }));
    await assert.rejects(
      backend.runTurn('p#vibe-loop', {
        prompt: 'review target.md',
        history: [],
        onChunk: () => {},
        onToolResult: () => {},
        cwd: workdir,
        requestPermission: async () => 'allow-once'
      }),
      /more than 5 tool calls in a single turn/
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
    rmSync(workdir, { recursive: true, force: true });
  }
});


test('bound OpenHands profile supplies model metadata without requiring or returning its legacy key', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-bound-openhands-profile-'));
  const saved = { HOME: process.env.HOME, GAH_BINARY: process.env.GAH_BINARY,
    LLM_MODEL: process.env.LLM_MODEL, LLM_BASE_URL: process.env.LLM_BASE_URL };
  const cli = join(dir, 'gah');
  try {
    process.env.HOME = dir;
    process.env.GAH_BINARY = cli;
    delete process.env.LLM_MODEL;
    delete process.env.LLM_BASE_URL;
    writeFileSync(cli, '#!/bin/sh\n'+ "echo '{\"profiles\":{\"bound\":{\"oh_profile\":\"work\"}}}'\n", { mode: 0o755 });
    mkdirSync(join(dir, '.openhands', 'profiles'), { recursive: true });
    const profile = join(dir, '.openhands', 'profiles', 'work.json');
    writeFileSync(profile, JSON.stringify({ model: 'openai/gpt', base_url: 'https://source.example/v1' }));
    const spec = openhandsBackendSpec({ instanceCredential: true });
    assert.deepEqual(await spec.spawnEnv?.('bound'), { LLM_MODEL: 'openai/gpt', LLM_BASE_URL: 'https://source.example/v1', OPENHANDS_SUPPRESS_BANNER: '1' });
    writeFileSync(profile, JSON.stringify({ model: 'openai/gpt', base_url: 'https://source.example/v1', api_key: 'old-profile-key' }));
    assert.equal((await spec.spawnEnv?.('bound'))?.LLM_API_KEY, undefined);
  } finally {
    for (const [name, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[name]; else process.env[name] = value;
    }
    rmSync(dir, { recursive: true, force: true });
  }
});


test('a named Vibe instance derives its Python bridge from the selected launcher', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-vibe-selected-launcher-'));
  const selected = join(dir, 'selected-vibe');
  try {
    writeFileSync(selected, '#!/selected/install/bin/python3\n');
    assert.equal(vibeBackendSpec({ executable: selected }).turnArgs()[0], '/selected/install/bin/python3');
    writeFileSync(selected, '#!/usr/bin/env python3\n');
    assert.throws(() => vibeBackendSpec({ executable: selected }).turnArgs(), /absolute Python interpreter/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('Cursor argv, authentication and quota errors use the headless interfaces', () => {
  const spec = cursorBackendSpec();
  assert.deepEqual(spec.turnArgs({ model: 'sonnet', prompt: 'hello', resumeId: 'chat-1' }),
    ['cursor-agent', '-p', '--output-format', 'stream-json', '--stream-partial-output', '--model', 'sonnet', '--resume', 'chat-1', 'hello']);
  assert.throws(() => spec.parseReply!({ stdout: '', stderr: 'Authentication required: secret detail', exitCode: 1 }),
    /^Error: Cursor backend unavailable: run cursor-agent login to authenticate, then retry\.$/);
  assert.throws(() => spec.parseReply!({ stdout: '', stderr: 'You have reached your request limit', exitCode: 1 }),
    error => isUsageLimitError(error));
  assert.throws(() => spec.parseReply!({ stdout: '{"type":"result","is_error":true,"result":"Usage limit exceeded","session_id":"private-id"}', stderr: '', exitCode: 1 }),
    error => error instanceof Error && isUsageLimitError(error) && !error.message.includes('private-id'));
  assert.equal(isUsageLimitError(new Error('Authentication required')), false);
});

// Synthetic fixture based on https://cursor.com/docs/cli/reference/output-format.
// A capture from an authenticated real CLI is still required for live verification.
const cursorStream = [
  { type: 'system', subtype: 'init', session_id: 'chat-1' },
  { type: 'assistant', timestamp_ms: 1, message: { content: [{ type: 'text', text: 'Hello ' }] } },
  { type: 'assistant', timestamp_ms: 2, model_call_id: 'call-1', message: { content: [{ type: 'text', text: 'Hello ' }] } },
  { type: 'assistant', timestamp_ms: 3, message: { content: [{ type: 'text', text: 'world' }] } },
  { type: 'assistant', message: { content: [{ type: 'text', text: 'Hello world' }] } },
  { type: 'result', subtype: 'success', is_error: false, result: 'Hello world', session_id: 'chat-1' }
].map(event => JSON.stringify(event)).join('\n');

test('Cursor streams split NDJSON deltas once and uses the terminal result', () => {
  const chunks: string[] = [];
  const stream = cursorBackendSpec().createStream!(text => chunks.push(text));
  for (const character of cursorStream) stream.feed(character);
  assert.deepEqual(stream.finish(), { reply: 'Hello world', sessionId: 'chat-1' });
  assert.deepEqual(chunks, ['Hello ', 'world']);
  const truncated = cursorBackendSpec().createStream!(() => {});
  truncated.feed('{"type":"system"}\n');
  assert.throws(() => truncated.finish(), /without a terminal result/);
  const malformed = cursorBackendSpec().createStream!(() => {});
  assert.doesNotThrow(() => malformed.feed('not-json\n'));
  assert.throws(() => malformed.finish(), SyntaxError);
});

test('Cursor persists chat metadata, resumes after adapter restart, and replays after failed resume', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-cursor-'));
  const previous = process.env.GAH_CHAT_STATE_DIR;
  try {
    process.env.GAH_CHAT_STATE_DIR = dir;
    const executable = join(dir, 'cursor-agent');
    const record = join(dir, 'argv.jsonl');
    writeFileSync(executable, `#!${execPath}
const fs = require('node:fs');
const args = process.argv.slice(2);
fs.appendFileSync(${JSON.stringify(record)}, JSON.stringify({args, cwd:process.cwd()})+'\\n');
if(args.includes('--list-models')) { fs.writeSync(1, 'sonnet - Sonnet\\n'); process.exit(0); }
if(args.includes('--resume') && fs.existsSync(${JSON.stringify(join(dir, 'fail'))})) { fs.writeSync(2, 'Chat not found'); process.exit(1); }
fs.writeSync(1, ${JSON.stringify(cursorStream)});
`, { mode: 0o755 });
    const input = { prompt: 'hello', history: [] as ChatTranscriptTurn[], cwd: dir, onChunk: () => {}, onToolResult: () => {} };
    const first = createHeadlessBackend(cursorBackendSpec(executable));
    assert.equal((await first.runTurn('p#session', input)).reply, 'Hello world');
    const metadata = JSON.parse(readFileSync(join(dir, 'backend-sessions', 'p%23session-cursor.json'), 'utf8'));
    assert.equal(metadata.sessionId, 'chat-1');
    const second = createHeadlessBackend(cursorBackendSpec(executable));
    const models = await second.listModels('p#session');
    assert.equal(models.models[0].id, 'sonnet');
    await second.listModels('p#session');
    await second.runTurn('p#session', { ...input, prompt: 'next' });
    writeFileSync(join(dir, 'fail'), '');
    await second.runTurn('p#session', { ...input, prompt: 'retry', history: [{ role: 'assistant', text: 'past', timestamp: 1 }] });
    const calls = readFileSync(record, 'utf8').trim().split('\n').map(line => JSON.parse(line));
    assert.equal(calls.filter(call => call.args.includes('--list-models')).length, 1);
    const turns = calls.filter(call => !call.args.includes('--list-models'));
    assert.equal(turns[0].cwd, dir);
    assert.ok(turns[1].args.includes('--resume'));
    assert.equal(turns[1].args.at(-1), 'next');
    assert.ok(!turns[3].args.includes('--resume'));
    assert.equal(turns[3].args.at(-1), 'assistant: past\n\nuser: retry');
  } finally {
    if (previous === undefined) delete process.env.GAH_CHAT_STATE_DIR; else process.env.GAH_CHAT_STATE_DIR = previous;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('Cursor cancellation terminates descendants and rejects as cancelled', async () => {
  if (process.platform === 'win32') return;
  const dir = mkdtempSync(join(tmpdir(), 'gah-cursor-cancel-'));
  const previous = process.env.GAH_CHAT_STATE_DIR;
  try {
    process.env.GAH_CHAT_STATE_DIR = dir;
    const executable = join(dir, 'cursor-agent');
    const marker = join(dir, 'descendant-survived');
    writeFileSync(executable, `#!/bin/sh
(sleep 1; touch '${marker}') &
echo '{"type":"assistant","timestamp_ms":1,"message":{"content":[{"type":"text","text":"ready"}]}}'
wait
`, { mode: 0o755 });
    const backend = createHeadlessBackend(cursorBackendSpec(executable));
    let ready!: () => void;
    const started = new Promise<void>(resolve => { ready = resolve; });
    const turn = backend.runTurn('cancel', { prompt: 'go', history: [], cwd: dir, onChunk: ready, onToolResult: () => {} });
    await started;
    await backend.cancelTurn('cancel');
    await assert.rejects(turn, /Turn cancelled/);
    await new Promise(resolve => setTimeout(resolve, 1200));
    assert.throws(() => readFileSync(marker), /ENOENT/);
  } finally {
    if (previous === undefined) delete process.env.GAH_CHAT_STATE_DIR; else process.env.GAH_CHAT_STATE_DIR = previous;
    rmSync(dir, { recursive: true, force: true });
  }
});
