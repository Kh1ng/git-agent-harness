import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { ApnsNotifications, type ApnsRequest } from './apns.js';
import type { ActivityEvent } from '@git-agent-harness/contracts';

const token = (character: string) => character.repeat(64);

function fixture(response: (request: ApnsRequest) => { status: number; reason?: string } = () => ({ status: 200 })) {
  const directory = mkdtempSync(join(tmpdir(), 'gah-apns-'));
  const keyPath = join(directory, 'AuthKey.p8');
  const devicesPath = join(directory, 'devices.json');
  const { privateKey } = generateKeyPairSync('ec', { namedCurve: 'P-256' });
  writeFileSync(keyPath, privateKey.export({ type: 'pkcs8', format: 'pem' }));
  const requests: ApnsRequest[] = [];
  let clock = Date.parse('2026-09-25T12:00:00Z');
  const service = new ApnsNotifications(
    { keyPath, keyId: 'KEY123', teamId: 'TEAM123', bundleId: 'com.kh1ng.gah.controller', environment: 'sandbox' },
    devicesPath,
    async (_host, request) => { requests.push(request); return response(request); },
    () => clock
  );
  return { directory, devicesPath, requests, service, advance: (milliseconds: number) => { clock += milliseconds; } };
}

test('APNs alert uses token auth, shared payload, collapse id, private storage, and prunes 410', async () => {
  const setup = fixture(() => ({ status: 410, reason: 'Unregistered' }));
  try {
    setup.service.register({ token: token('a'), label: 'iPhone' });
    setup.service.register({ token: token('b'), label: 'iPad' });
    assert.equal(statSync(setup.devicesPath).mode & 0o777, 0o600);
    const event: ActivityEvent = {
      id: 'chat:gah:s1:1:complete', occurredAt: '2026-09-25T12:00:00Z', profile: 'gah', sessionId: 's1',
      kind: 'chat_turn_completed', severity: 'success', title: 'gah: reply ready', message: 'Done.'
    };
    await setup.service.deliverActivity(event);
    assert.equal(setup.requests.length, 2);
    assert.match(setup.requests[0].headers.authorization, /^bearer [^.]+\.[^.]+\.[^.]+$/);
    assert.equal(setup.requests[0].headers['apns-collapse-id'], event.id);
    assert.equal(setup.requests[0].headers['apns-topic'], 'com.kh1ng.gah.controller');
    assert.deepEqual(Object.keys(setup.requests[0].payload).sort(), ['aps', 'body', 'id', 'title', 'url']);
    assert.deepEqual(setup.service.list(), { count: 0 });
  } finally { rmSync(setup.directory, { recursive: true, force: true }); }
});

test('Live Activity starts remotely, rate-limits updates, and always ends', async () => {
  const setup = fixture();
  try {
    setup.service.register({
      token: token('a'), pushToStartToken: token('b'),
      liveActivity: { profile: 'gah', sessionId: 's1', token: token('c') }
    });
    const base = { profile: 'gah', sessionId: 's1', turn: 1, occurredAt: '2026-09-25T12:00:00Z', backend: 'codex', model: 'gpt-5' } as const;
    await setup.service.deliverChatLifecycle({ ...base, phase: 'start' });
    await setup.service.deliverChatLifecycle({ ...base, phase: 'tool', tool: 'cargo test' });
    await setup.service.deliverChatLifecycle({ ...base, phase: 'permission', permissionId: 'permission-1', tool: 'shell' });
    assert.equal(setup.requests.length, 3);
    assert.equal((setup.requests[2].payload as { aps: { 'content-state': { state: string } } }).aps['content-state'].state, 'waiting for permission');
    await setup.service.deliverChatLifecycle({ ...base, phase: 'permission', permissionId: 'permission-1', tool: 'shell' });
    setup.advance(1_000);
    await setup.service.deliverChatLifecycle({ ...base, phase: 'tool', tool: 'cargo fmt' });
    await setup.service.deliverChatLifecycle({ ...base, phase: 'permission', permissionId: 'permission-2', tool: 'shell' });
    assert.equal(setup.requests.length, 4);
    await setup.service.deliverChatLifecycle({ ...base, phase: 'end', outcome: 'complete' });
    assert.equal(setup.requests.length, 5);
    assert.equal(setup.requests[0].token, token('b'));
    assert.equal((setup.requests[0].payload as { aps: { event: string } }).aps.event, 'start');
    assert.equal(setup.requests[0].headers['apns-topic'], 'com.kh1ng.gah.controller.push-type.liveactivity');
    assert.deepEqual(setup.requests.slice(1).map((request) => (request.payload as { aps: { event: string } }).aps.event), ['update', 'update', 'update', 'end']);
    const dismissal = (setup.requests.at(-1)!.payload as { aps: { 'dismissal-date': number } }).aps['dismissal-date'];
    assert.equal(dismissal, Math.floor((Date.parse('2026-09-25T12:00:01Z') + 15 * 60_000) / 1_000));
  } finally { rmSync(setup.directory, { recursive: true, force: true }); }
});

test('revoking a paired device removes only its APNs registration', () => {
  const setup = fixture();
  try {
    setup.service.register({ token: token('a') }, 'device-1');
    setup.service.register({ token: token('b') }, 'device-2');
    setup.service.removeForDevice('device-1');
    assert.deepEqual(setup.service.list(), { count: 1 });
    const stored = JSON.parse(readFileSync(setup.devicesPath, 'utf8')) as { deviceId?: string }[];
    assert.deepEqual(stored.map((entry) => entry.deviceId), ['device-2']);
  } finally { rmSync(setup.directory, { recursive: true, force: true }); }
});
