import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { createAcpBackend } from './managerChat/acpAdapter.js';

const FAKE_ACP_SOURCE = String.raw`
const fs = require('node:fs');
const readline = require('node:readline');

const mode = process.argv[2];
const spawnCountPath = process.argv[3];
let spawnNumber = 1;
if (spawnCountPath) {
  const previous = fs.existsSync(spawnCountPath) ? Number(fs.readFileSync(spawnCountPath, 'utf8')) : 0;
  spawnNumber = previous + 1;
  fs.writeFileSync(spawnCountPath, String(spawnNumber));
}
let promptNumber = 0;
const lines = readline.createInterface({ input: process.stdin });

function respond(message) {
  process.stdout.write(JSON.stringify(message) + '\n');
}

lines.on('line', (line) => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') {
    respond({ jsonrpc: '2.0', id: request.id, result: { protocolVersion: request.params.protocolVersion } });
    return;
  }
  if (request.method === 'session/new') {
    respond({ jsonrpc: '2.0', id: request.id, result: { sessionId: 'fake-session' } });
    return;
  }
  if (request.method === 'session/prompt' && mode === 'detail-less-error') {
    process.stderr.write('SECRET-START\n' + 'x'.repeat(5000) + '\nDIAGNOSTIC-END\n', () => {
      respond({ jsonrpc: '2.0', id: request.id, error: { code: -32603, message: 'Internal error' } });
      setTimeout(() => process.exit(17), 25);
    });
    return;
  }
  if (request.method === 'session/prompt' && mode === 'detailed-error') {
    respond({
      jsonrpc: '2.0',
      id: request.id,
      error: { code: -32603, message: 'Internal error', data: { message: 'Useful nested detail' } }
    });
    setTimeout(() => process.exit(0), 25);
    return;
  }
  if (request.method === 'session/prompt' && mode === 'reconnect-sequence') {
    promptNumber += 1;
    if (spawnNumber === 1 && promptNumber !== 2) {
      respond({ jsonrpc: '2.0', id: request.id, error: { code: -32603, message: 'Internal error' } });
      return;
    }
    const text = spawnNumber === 1
      ? 'recovered-on-original-child'
      : request.params.prompt[0].text.includes('user: before') ? 'rehydrated-on-new-child' : 'history-missing';
    respond({
      jsonrpc: '2.0',
      method: 'session/update',
      params: {
        sessionId: 'fake-session',
        update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text } }
      }
    });
    respond({ jsonrpc: '2.0', id: request.id, result: { stopReason: 'end_turn' } });
    if (spawnNumber > 1) setTimeout(() => process.exit(0), 25);
  }
});
`;

function fakeAcpScript(): { dir: string; path: string } {
  const dir = mkdtempSync(join(tmpdir(), 'gah-fake-acp-'));
  const scriptPath = join(dir, 'fake-acp.cjs');
  writeFileSync(scriptPath, FAKE_ACP_SOURCE);
  return { dir, path: scriptPath };
}

const emptyTurnInput = {
  prompt: 'hello',
  history: [],
  onChunk: () => undefined,
  onToolResult: () => undefined
};

test('detail-less ACP rejection surfaces code and bounded child diagnostics', async () => {
  const fake = fakeAcpScript();
  try {
    const backend = createAcpBackend('Codex', () => ({
      command: process.execPath,
      args: [fake.path, 'detail-less-error']
    }));

    await assert.rejects(
      backend.runTurn('profile', emptyTurnInput),
      (error: unknown) => {
        assert.ok(error instanceof Error);
        assert.match(error.message, /^Internal error \[ACP code=-32603;/);
        assert.match(error.message, /stderr tail=.*DIAGNOSTIC-END/);
        assert.doesNotMatch(error.message, /SECRET-START/);
        assert.ok(error.message.length < 4_500, `diagnostic error was ${error.message.length} characters`);
        return true;
      }
    );
  } finally {
    await new Promise((resolve) => setTimeout(resolve, 50));
    rmSync(fake.dir, { recursive: true, force: true });
  }
});

test('ACP rejection preserves nested detail without diagnostic decoration', async () => {
  const fake = fakeAcpScript();
  try {
    const backend = createAcpBackend('Codex', () => ({
      command: process.execPath,
      args: [fake.path, 'detailed-error']
    }), { consecutiveFailureReconnectThreshold: 2 });

    await assert.rejects(
      backend.runTurn('profile', emptyTurnInput),
      (error: unknown) => error instanceof Error && error.message === 'Useful nested detail'
    );
  } finally {
    await new Promise((resolve) => setTimeout(resolve, 50));
    rmSync(fake.dir, { recursive: true, force: true });
  }
});

test('Codex evicts after two consecutive ACP failures and rehydrates on the next turn', async () => {
  const fake = fakeAcpScript();
  const spawnCountPath = join(fake.dir, 'spawn-count');
  const durableHistory = [
    { role: 'user' as const, text: 'before', timestamp: 1 },
    { role: 'assistant' as const, text: 'durable answer', timestamp: 2 }
  ];
  try {
    const backend = createAcpBackend('Codex', () => ({
      command: process.execPath,
      args: [fake.path, 'reconnect-sequence', spawnCountPath]
    }), { consecutiveFailureReconnectThreshold: 2 });

    await assert.rejects(
      backend.runTurn('profile', { ...emptyTurnInput, prompt: 'fails once', history: durableHistory }),
      /consecutive failures=1\/2/
    );
    const recovered = await backend.runTurn('profile', {
      ...emptyTurnInput,
      prompt: 'succeeds and resets the counter',
      history: durableHistory
    });
    assert.equal(recovered.reply, 'recovered-on-original-child');

    const currentHistory = [
      ...durableHistory,
      { role: 'user' as const, text: 'succeeds and resets the counter', timestamp: 3 },
      { role: 'assistant' as const, text: recovered.reply, timestamp: 4 }
    ];
    await assert.rejects(
      backend.runTurn('profile', { ...emptyTurnInput, prompt: 'fails after success', history: currentHistory }),
      /consecutive failures=1\/2/
    );
    await assert.rejects(
      backend.runTurn('profile', { ...emptyTurnInput, prompt: 'second consecutive failure', history: currentHistory }),
      /consecutive failures=2\/2; connection evicted/
    );

    const rehydrated = await backend.runTurn('profile', {
      ...emptyTurnInput,
      prompt: 'next turn',
      history: currentHistory
    });
    assert.equal(rehydrated.reply, 'rehydrated-on-new-child');
    assert.equal(Number(readFileSync(spawnCountPath, 'utf8')), 2);
  } finally {
    await new Promise((resolve) => setTimeout(resolve, 50));
    rmSync(fake.dir, { recursive: true, force: true });
  }
});
