import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ActivityEvent } from '@git-agent-harness/contracts';
import { channelDelivery, commandDelivery, deliverToAll } from './notifyDelivery.js';

const BASE = 'https://central.example';

function event(overrides: Partial<ActivityEvent> = {}): ActivityEvent {
  return {
    id: 'chat:gah:s1:1:chat_turn_completed',
    occurredAt: '2026-09-26T12:00:00Z',
    profile: 'gah',
    sessionId: 's1',
    kind: 'chat_turn_completed',
    severity: 'success',
    title: 'gah: reply ready',
    message: 'Done.\nTests pass.',
    ...overrides
  };
}

function withDirectory(run: (directory: string) => Promise<void>): Promise<void> {
  const directory = mkdtempSync(join(tmpdir(), 'gah-notify-delivery-'));
  return run(directory).finally(() => rmSync(directory, { recursive: true, force: true }));
}

test('the command hook gets one line per notifiable event and skips the rest', () => withDirectory(async (directory) => {
  const log = join(directory, 'hook.log');
  const deliver = commandDelivery(BASE, { GAH_NOTIFY_COMMAND: `cat >> ${log}` })!;
  await deliver(event());
  await deliver(event({ id: 'node:x', kind: 'node_back', title: 'mac is back online' }));
  assert.equal(
    readFileSync(log, 'utf8'),
    '[gah] gah: reply ready: Done. Tests pass. https://central.example/?page=chat&profile=gah&chat=s1\n'
  );
}));

test('the liveness variable still works as an alias for the command hook', () => withDirectory(async (directory) => {
  const log = join(directory, 'hook.log');
  const deliver = commandDelivery(BASE, { GAH_NODE_LIVENESS_NOTIFY_COMMAND: `cat >> ${log}` });
  assert.ok(deliver);
  await deliver(event({ id: 'node:y', kind: 'node_offline', profile: null, sessionId: null, title: 'mac is offline', message: 'no answer' }));
  assert.match(readFileSync(log, 'utf8'), /^\[gah\] mac is offline: no answer https:\/\/central\.example\/\?page=events\n$/);
  assert.equal(commandDelivery(BASE, {}), undefined);
}));

test('the channel sends Node-originated events through gah notify-send, not controller-log events', () => withDirectory(async (directory) => {
  const log = join(directory, 'argv.log');
  const gah = join(directory, 'gah');
  writeFileSync(gah, `#!/bin/sh\nprintf '%s\\n' "$@" >> ${log}\n`);
  chmodSync(gah, 0o755);
  const deliver = channelDelivery(BASE, () => gah);
  await deliver(event({ id: 'controller:abc', kind: 'dispatch_failed' }));
  await deliver(event({ id: 'quota:abc', kind: 'quota_near_limit' }));
  assert.equal(existsSync(log), false);
  await deliver(event());
  assert.deepEqual(readFileSync(log, 'utf8').trim().split('\n'), [
    'notify-send',
    '--title', 'gah: reply ready',
    '--message', 'Done.',
    'Tests pass.',
    '--url', 'https://central.example/?page=chat&profile=gah&chat=s1'
  ]);
}));

test('one failing delivery method does not stop the others', async () => {
  const delivered: string[] = [];
  await deliverToAll([
    async () => { throw new Error('push service down'); },
    undefined,
    (item) => { delivered.push(item.id); }
  ])(event());
  assert.deepEqual(delivered, ['chat:gah:s1:1:chat_turn_completed']);
});
