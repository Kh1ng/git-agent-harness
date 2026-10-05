import assert from 'node:assert/strict';
import { test } from 'node:test';
import { appendFileSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { factoryRunOutput, parseRunLine, readRunOutput } from './factoryRunOutput.js';

const RUN = '8c4013e4-2937-4bac-b258-115ae2b3d7e1';
const line = (value: unknown) => `${JSON.stringify(value)}\n`;

test('parseRunLine reads Codex items, Claude messages and unknown lines', () => {
  assert.deepEqual(parseRunLine(line({ type: 'item.completed', item: { type: 'agent_message', text: 'I will trace the loop.' } })), [{ kind: 'message', text: 'I will trace the loop.' }]);
  assert.deepEqual(parseRunLine(line({ type: 'item.started', item: { type: 'command_execution', command: 'cargo test', aggregated_output: '' } })),
    [{ kind: 'command', text: 'cargo test', output: null, status: 'running', exit_code: null }]);
  const failed = parseRunLine(line({ type: 'item.completed', item: { type: 'command_execution', command: 'cargo test', aggregated_output: 'x'.repeat(9000), exit_code: 101 } }))[0];
  assert.equal(failed.status, 'failed');
  assert.equal(failed.exit_code, 101);
  assert.ok(failed.output!.startsWith('…') && failed.output!.length < 4100);
  assert.deepEqual(parseRunLine(line({ type: 'item.completed', item: { type: 'file_change', changes: [{ path: 'src/a.rs', kind: 'update' }] } })), [{ kind: 'file_change', text: 'update src/a.rs', status: 'completed' }]);
  assert.deepEqual(parseRunLine(line({ type: 'assistant', message: { content: [{ type: 'text', text: 'Looking.' }, { type: 'tool_use', name: 'Bash', input: { command: 'ls' } }] } })),
    [{ kind: 'message', text: 'Looking.' }, { kind: 'command', text: 'Bash {"command":"ls"}', status: 'running' }]);
  assert.deepEqual(parseRunLine('Reading additional input from stdin...'), [{ kind: 'raw', text: 'Reading additional input from stdin...' }]);
  assert.deepEqual(parseRunLine(line({ type: 'turn.started' })), []);
  assert.deepEqual(parseRunLine('   '), []);
});

test('readRunOutput follows a growing log by offset and only returns whole lines', () => {
  const file = join(mkdtempSync(join(tmpdir(), 'gah-run-output-')), 'backend-output.log');
  writeFileSync(file, line({ type: 'item.started', item: { type: 'command_execution', command: 'ls' } })
    + line({ type: 'item.completed', item: { type: 'command_execution', command: 'ls', aggregated_output: 'a\nb', exit_code: 0 } })
    + '{"type":"item.completed","item":{"type":"agent_mess');
  const first = readRunOutput(file, 1, 0);
  // The started echo is dropped once the result is there; the half-written line waits.
  assert.deepEqual(first.events, [{ kind: 'command', text: 'ls', output: 'a\nb', status: 'completed', exit_code: 0 }]);
  assert.equal(first.truncated, false);
  appendFileSync(file, 'age","text":"Done."}}\n');
  const second = readRunOutput(file, 1, first.next);
  assert.deepEqual(second.events, [{ kind: 'message', text: 'Done.' }]);
  assert.deepEqual(readRunOutput(file, 1, second.next).events, []);
});

test('factoryRunOutput only serves logs a running loop holds open', () => {
  const file = join(mkdtempSync(join(tmpdir(), 'gah-run-output-')), 'backend-output.log');
  writeFileSync(file, line({ type: 'item.completed', item: { type: 'agent_message', text: 'Hello.' } }));
  const logs = new Map([[RUN, { file, attempt: 2 }]]);
  const output = factoryRunOutput(RUN, 0, logs);
  assert.equal(output.found, true);
  assert.equal(output.attempt, 2);
  assert.equal(output.events[0].text, 'Hello.');
  assert.equal(factoryRunOutput('00000000-0000-4000-8000-000000000000', 0, logs).found, false);
  assert.equal(factoryRunOutput('../../etc/passwd', 0, logs).found, false);
});

test('a line longer than the read window is still read whole', () => {
  const file = join(mkdtempSync(join(tmpdir(), 'gah-run-output-')), 'backend-output.log');
  const big = 'y'.repeat(600 * 1024);
  writeFileSync(file, line({ type: 'item.completed', item: { type: 'agent_message', text: 'Before.' } })
    + line({ type: 'item.completed', item: { type: 'command_execution', command: 'cargo test', aggregated_output: big, exit_code: 0 } })
    + line({ type: 'item.completed', item: { type: 'agent_message', text: 'After.' } }));
  const events = [];
  let after = 0;
  for (let round = 0; round < 10; round++) {
    const output = readRunOutput(file, 1, after);
    events.push(...output.events);
    if (output.next === after) break;
    after = output.next;
  }
  // The first read of a long log starts near its end, inside the big line, and resumes after it.
  assert.deepEqual(events.map((event) => event.text), ['After.']);
  // Resuming exactly at the big line widens the window until the whole line fits.
  const atBigLine = Buffer.byteLength(line({ type: 'item.completed', item: { type: 'agent_message', text: 'Before.' } }));
  const big1 = readRunOutput(file, 1, atBigLine);
  assert.equal(big1.events[0].kind, 'command');
  assert.equal(big1.events[0].status, 'completed');
  assert.ok(big1.events[0].output!.length < 4100);
});
