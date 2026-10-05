import assert from 'node:assert/strict';
import { test } from 'node:test';
import { appendFileSync, mkdirSync, mkdtempSync, statSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { factoryRunOutput, findRunLogOnDisk, parseRunLine, readRunOutput } from './factoryRunOutput.js';

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
  assert.deepEqual(parseRunLine(line({ type: 'turn.failed', error: { message: 'Selected model is at capacity.' } })),
    [{ kind: 'command', text: 'The agent\'s turn failed', output: 'Selected model is at capacity.', status: 'failed' }]);
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

test('factoryRunOutput serves logs a loop holds open, then the run directory once it has closed them', () => {
  const sessions = join(mkdtempSync(join(tmpdir(), 'gah-run-output-')), 'sessions');
  mkdirSync(join(sessions, RUN, 'attempt-1'), { recursive: true });
  const file = join(sessions, RUN, 'attempt-1', 'backend-output.log');
  writeFileSync(file, line({ type: 'item.completed', item: { type: 'agent_message', text: 'Hello.' } }));
  const held = { logs: new Map([[RUN, { file, attempt: 2 }]]), roots: new Set<string>() };
  const output = factoryRunOutput(RUN, 0, held);
  assert.equal(output.found, true);
  assert.equal(output.attempt, 2);
  assert.equal(output.events[0].text, 'Hello.');
  assert.equal(factoryRunOutput('00000000-0000-4000-8000-000000000000', 0, held).found, false);
  assert.equal(factoryRunOutput('../../etc/passwd', 0, { logs: new Map(), roots: new Set([sessions]) }).found, false);

  // The loop has moved on: the run is found under a sessions directory it was seen using,
  // and a review's output (written later) is the step to show.
  mkdirSync(join(sessions, RUN, 'review-attempt-1'), { recursive: true });
  const review = join(sessions, RUN, 'review-attempt-1', 'review-stdout.log');
  writeFileSync(review, line({ type: 'item.completed', item: { type: 'agent_message', text: 'Reviewing.' } }));
  utimesSync(review, new Date(Date.now() + 5000), new Date(Date.now() + 5000));
  const closed = { logs: new Map(), roots: new Set([sessions]) };
  assert.equal(factoryRunOutput(RUN, 0, closed).events[0].text, 'Reviewing.');
  assert.deepEqual(findRunLogOnDisk(RUN, [sessions]), { file: review, attempt: 1 });
  assert.equal(findRunLogOnDisk('00000000-0000-4000-8000-000000000000', [sessions]), null);
});

test('a run id that resolves outside its sessions root is not served', () => {
  const base = mkdtempSync(join(tmpdir(), 'gah-run-output-'));
  const outside = join(base, 'outside', RUN);
  mkdirSync(join(outside, 'attempt-1'), { recursive: true });
  const file = join(outside, 'attempt-1', 'backend-output.log');
  writeFileSync(file, line({ type: 'item.completed', item: { type: 'agent_message', text: 'Outside.' } }));
  assert.deepEqual(findRunLogOnDisk(RUN, [join(base, 'outside')]), { file, attempt: 1 });
  // The `..` puts a real directory with a real log outside the root: containment, not a failed read, is what stops it.
  assert.equal(findRunLogOnDisk(`../outside/${RUN}`, [join(base, 'sessions')]), null);
  assert.equal(factoryRunOutput(`../outside/${RUN}`, 0, { logs: new Map(), roots: new Set([join(base, 'sessions')]) }).found, false);
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
  // A full read starts at the beginning however long the log is.
  const whole: string[] = [];
  for (let offset = 0, round = 0; round < 10; round++) {
    const output = readRunOutput(file, 1, offset, true);
    whole.push(...output.events.map((event) => event.text));
    assert.equal(output.truncated, false);
    if (output.next === offset) break;
    offset = output.next;
  }
  assert.deepEqual(whole, ['Before.', 'cargo test', 'After.']);
  // Resuming exactly at the big line widens the window until the whole line fits.
  const atBigLine = Buffer.byteLength(line({ type: 'item.completed', item: { type: 'agent_message', text: 'Before.' } }));
  const big1 = readRunOutput(file, 1, atBigLine);
  assert.equal(big1.events[0].kind, 'command');
  assert.equal(big1.events[0].status, 'completed');
  assert.ok(big1.events[0].output!.length < 4100);
});

test('an offset into one log is not applied to the next log the run writes', () => {
  const sessions = join(mkdtempSync(join(tmpdir(), 'gah-run-output-')), 'sessions');
  mkdirSync(join(sessions, RUN, 'attempt-1'), { recursive: true });
  const first = join(sessions, RUN, 'attempt-1', 'backend-output.log');
  writeFileSync(first, line({ type: 'item.completed', item: { type: 'agent_message', text: 'x'.repeat(500) } }));
  const one = factoryRunOutput(RUN, 0, { logs: new Map([[RUN, { file: first, attempt: 1 }]]), roots: new Set() });
  assert.equal(one.log, 'attempt-1/backend-output.log');

  // The review starts: a shorter log than the offset the viewer holds.
  mkdirSync(join(sessions, RUN, 'review-attempt-1'), { recursive: true });
  const review = join(sessions, RUN, 'review-attempt-1', 'review-stdout.log');
  writeFileSync(review, line({ type: 'item.completed', item: { type: 'agent_message', text: 'Reviewing.' } }) + 'y'.repeat(600) + '\n');
  const two = factoryRunOutput(RUN, one.next, { logs: new Map([[RUN, { file: review, attempt: 1 }]]), roots: new Set() }, false, one.log);
  assert.equal(two.log, 'review-attempt-1/review-stdout.log');
  assert.equal(two.events[0].text, 'Reviewing.', 'the new log is read from its start');
});

test('the partial first line of a long log is skipped by bytes, not decoded characters', () => {
  const file = join(mkdtempSync(join(tmpdir(), 'gah-run-output-')), 'backend-output.log');
  // Multi-byte characters straddle the read window's start.
  writeFileSync(file, `${'é'.repeat(200 * 1024)}\n${line({ type: 'item.completed', item: { type: 'agent_message', text: 'After.' } })}`);
  const output = readRunOutput(file, 1, 0);
  assert.equal(output.truncated, true);
  assert.deepEqual(output.events.map((event) => event.text), ['After.']);
  assert.equal(readRunOutput(file, 1, output.next).events.length, 0, 'next is the exact end of the log');
  assert.equal(output.next, statSync(file).size);
});
