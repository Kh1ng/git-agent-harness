import assert from 'node:assert/strict';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { MessagingBridge, type BridgeOperator, type TelegramButton, type TelegramTransport } from './messagingBridge.js';
import { createServer } from '../server.js';
import { resetCachedCoordinatorIdentity } from '../coordinatorIdentity.js';
import { RegistryService } from '../registryService.js';
import telegramFixture from '../../tests/fixtures/telegram/message-with-document.json' with { type: 'json' };

class FakeTelegram implements TelegramTransport {
  sent: { chatId: string; text: string; buttons: TelegramButton[] }[] = [];
  callbacks: { id: string; text: string }[] = [];
  attachment = '';
  failSends = 0;
  onSend?: (buttons: TelegramButton[]) => void;

  async send(chatId: string, text: string, buttons: TelegramButton[] = []): Promise<void> {
    if (this.failSends-- > 0) throw new Error('gateway offline');
    this.sent.push({ chatId, text, buttons });
    this.onSend?.(buttons);
  }

  async answerCallback(id: string, text: string): Promise<void> {
    this.callbacks.push({ id, text });
  }

  async loadTextDocument(): Promise<string> {
    return this.attachment;
  }
}

function message(updateId: number, text: string, extra: object = {}) {
  return { update_id: updateId, message: { from: { id: 42 }, chat: { id: -77 }, text, ...extra } };
}

function pairedBridge(options: ConstructorParameters<typeof MessagingBridge>[0] = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'gah-messaging-bridge-'));
  const transport = options.transport ?? new FakeTelegram();
  const bridge = new MessagingBridge({ stateDir: directory, telegramSecret: 'webhook-secret', transport, ...options });
  const operator = bridge.pair({ externalUserId: '42', chatId: '-77', role: 'owner', profiles: ['gah'] });
  return { bridge, directory, operator, transport: transport as FakeTelegram };
}

test('Telegram round trip authenticates, scopes, bounds, and redacts text attachments', async () => {
  let prompt = '';
  const fake = new FakeTelegram();
  fake.attachment = 'Authorization: Bearer top-secret\ntoken=also-secret';
  const { bridge, directory } = pairedBridge({
    transport: fake,
    sendTurn: async (input) => { prompt = input.text; return `done sk-${'x'.repeat(24)}`; }
  });
  try {
    assert.equal(bridge.telegramAuthenticated('wrong'), false);
    assert.equal(bridge.telegramAuthenticated('webhook-secret'), true);
    await bridge.handleTelegram(telegramFixture);
    assert.match(prompt, /Attachment notes\.txt/);
    assert.doesNotMatch(prompt, /top-secret|also-secret/);
    assert.match(prompt, /\[REDACTED:/);
    assert.equal(fake.sent.length, 1);
    assert.doesNotMatch(fake.sent[0].text, /sk-/);
    const audit = readFileSync(join(directory, 'audit.jsonl'), 'utf8');
    assert.match(audit, /update\.accepted/);
    assert.match(audit, /update\.delivered/);
    assert.doesNotMatch(audit, /Review this|top-secret/);
  } finally {
    rmSync(directory, { recursive: true });
  }
});

test('a repeated Telegram update retries delivery without duplicating the manager turn', async () => {
  let turns = 0;
  const fake = new FakeTelegram();
  fake.failSends = 1;
  const { bridge, directory } = pairedBridge({
    transport: fake,
    sendTurn: async () => { turns++; return 'durable answer'; }
  });
  const update = message(2, 'status please');
  try {
    await assert.rejects(bridge.handleTelegram(update), /gateway offline/);
    const retry = await bridge.handleTelegram(update);
    assert.equal(retry.duplicate, true);
    assert.equal(turns, 1);
    assert.equal(fake.sent.at(-1)?.text, 'durable answer');
  } finally {
    rmSync(directory, { recursive: true });
  }
});

test('only an owner-paired exact action callback can answer a manager permission once', async () => {
  const fake = new FakeTelegram();
  let releaseTurn!: () => void;
  const turnReleased = new Promise<void>((resolve) => { releaseTurn = resolve; });
  let offered!: (buttons: TelegramButton[]) => void;
  const actionOffered = new Promise<TelegramButton[]>((resolve) => { offered = resolve; });
  fake.onSend = (buttons) => { if (buttons.length > 0) offered(buttons); };
  const decisions: string[] = [];
  const { bridge, directory } = pairedBridge({
    transport: fake,
    sendTurn: async ({ requestId, onPermission }) => {
      await onPermission({
        type: 'manager.chat.permission', requestId, profile: 'gah', turn: 1,
        permissionId: 'permission-1', title: 'Run cargo test', locations: ['/repo'],
        options: [
          { optionId: 'allow-once', name: 'Allow once', kind: 'allow_once' },
          { optionId: 'allow-always', name: 'Allow always', kind: 'allow_always' },
          { optionId: 'reject-once', name: 'Reject', kind: 'reject_once' }
        ]
      });
      await turnReleased;
      return 'action complete';
    },
    respondPermission: async (_profile, _session, _permission, option) => {
      decisions.push(option);
      releaseTurn();
      return true;
    }
  });
  try {
    const pending = bridge.handleTelegram(message(3, 'Run the checks'));
    const buttons = await actionOffered;
    assert.deepEqual(buttons.map((button) => button.text), ['Allow once', 'Reject']);
    const token = buttons[0].data;
    const callback = { update_id: 4, callback_query: { id: 'callback-1', data: token, from: { id: 42 }, message: { chat: { id: -77 } } } };
    await bridge.handleTelegram(callback);
    await pending;
    assert.deepEqual(decisions, ['allow-once']);
    assert.equal(fake.callbacks[0]?.text, 'Manager action recorded.');
    const replay = await bridge.handleTelegram(callback);
    assert.equal(replay.duplicate, true);
    assert.deepEqual(decisions, ['allow-once']);
  } finally {
    rmSync(directory, { recursive: true });
  }
});

test('chat-only identities and ambiguous remote commands fail closed', async () => {
  let turns = 0;
  const decisions: string[] = [];
  const { bridge, directory, operator, transport } = pairedBridge({
    sendTurn: async ({ requestId, onPermission }) => {
      turns++;
      await onPermission({
        type: 'manager.chat.permission', requestId, profile: 'gah', turn: 1,
        permissionId: 'permission-chat', title: 'Write config', locations: [],
        options: [
          { optionId: 'allow-once', name: 'Allow', kind: 'allow_once' },
          { optionId: 'reject-once', name: 'Reject', kind: 'reject_once' }
        ]
      });
      return 'denied safely';
    },
    respondPermission: async (_profile, _session, _permission, option) => { decisions.push(option); return true; }
  });
  try {
    bridge.pair({ externalUserId: '42', chatId: '-77', role: 'chat', profiles: ['gah'] });
    await bridge.handleTelegram(message(5, '/compact'));
    assert.equal(turns, 0);
    assert.match(transport.sent.at(-1)?.text ?? '', /slash commands are disabled/);
    await bridge.handleTelegram(message(6, 'Change the config'));
    assert.deepEqual(decisions, ['reject-once']);
    assert.equal(transport.sent.some((entry) => /denied.*owner-paired/i.test(entry.text)), true);
    bridge.revoke(operator.id);
    await bridge.handleTelegram(message(7, 'hello'));
    assert.equal(turns, 1);
  } finally {
    rmSync(directory, { recursive: true });
  }
});

test('HTTP bridge requires the Telegram secret and owner mutation receipts for pair and revoke', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-messaging-http-'));
  const fake = new FakeTelegram();
  let turns = 0;
  const bridge = new MessagingBridge({
    stateDir: join(directory, 'bridge'),
    telegramSecret: 'http-secret',
    transport: fake,
    sendTurn: async () => { turns++; return 'round trip'; }
  });
  const priorIdentity = process.env.GAH_COORDINATOR_IDENTITY_PATH;
  const priorMutations = process.env.GAH_MUTATION_STORE_PATH;
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(directory, 'identity.json');
  process.env.GAH_MUTATION_STORE_PATH = join(directory, 'mutations');
  resetCachedCoordinatorIdentity();
  const server = http.createServer(createServer({ messagingBridge: bridge, registryService: new RegistryService(null) }));
  if (priorIdentity === undefined) delete process.env.GAH_COORDINATOR_IDENTITY_PATH;
  else process.env.GAH_COORDINATOR_IDENTITY_PATH = priorIdentity;
  if (priorMutations === undefined) delete process.env.GAH_MUTATION_STORE_PATH;
  else process.env.GAH_MUTATION_STORE_PATH = priorMutations;
  resetCachedCoordinatorIdentity();
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  try {
    const pairBody = JSON.stringify({ externalUserId: '42', chatId: '-77', role: 'owner', profiles: ['gah'] });
    assert.equal((await fetch(`${origin}/api/manager-chat/bridge/operators`, {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: pairBody
    })).status, 400);
    const paired = await fetch(`${origin}/api/manager-chat/bridge/operators`, {
      method: 'POST', headers: { 'content-type': 'application/json', 'idempotency-key': 'pair-telegram-0001' }, body: pairBody
    });
    assert.equal(paired.status, 201);
    const operatorId = ((await paired.json()) as { operator: BridgeOperator }).operator.id;
    assert.equal((await fetch(`${origin}/api/manager-chat/bridge/telegram`, {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(message(10, 'hello'))
    })).status, 401);
    const delivered = await fetch(`${origin}/api/manager-chat/bridge/telegram`, {
      method: 'POST', headers: { 'content-type': 'application/json', 'x-telegram-bot-api-secret-token': 'http-secret' }, body: JSON.stringify(message(10, 'hello'))
    });
    assert.equal(delivered.status, 202);
    assert.equal(turns, 1);
    const revoked = await fetch(`${origin}/api/manager-chat/bridge/operators/revoke`, {
      method: 'POST', headers: { 'content-type': 'application/json', 'idempotency-key': 'revoke-telegram-01' }, body: JSON.stringify({ operatorId })
    });
    assert.equal(revoked.status, 200);
    await fetch(`${origin}/api/manager-chat/bridge/telegram`, {
      method: 'POST', headers: { 'content-type': 'application/json', 'x-telegram-bot-api-secret-token': 'http-secret' }, body: JSON.stringify(message(11, 'hello again'))
    });
    assert.equal(turns, 1);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(directory, { recursive: true });
  }
});
