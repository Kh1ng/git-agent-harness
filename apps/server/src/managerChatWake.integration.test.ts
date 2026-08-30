import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import { setChatEventPublishers, type UpdatedPublish } from './managerChat/ManagerChatManager.js';
import { readLog } from './managerChat/sessionLog.js';
import { createServer } from './server.js';

const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures');

test('two quick manager wakes share one durable backend session', { timeout: 15_000 }, async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-manager-wake-'));
  const stateDir = join(root, 'chat');
  const profile = `wake-${Date.now()}`;
  const savedEnv = { ...process.env };
  process.env.PATH = `${join(fixtures, 'hermes')}:${process.env.PATH}`;
  process.env.GAH_CHAT_STATE_DIR = stateDir;
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(root, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(root, 'manager-chat.json');
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(root, 'coordinator-identity.json');
  delete process.env.TDAI_GATEWAY_URL;
  delete process.env.TDAI_GATEWAY_API_KEY;
  resetCachedCoordinatorIdentity();
  const updates: Parameters<UpdatedPublish>[0][] = [];
  setChatEventPublishers({ updated: (event) => updates.push(event) });

  const app = createServer({
    runProfileList: async () => [{ name: profile, repo_id: 'stable-repo-id' } as ProfileSummary]
  });
  const server = http.createServer(app);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const baseUrl = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const wake = (instruction: string) => fetch(`${baseUrl}/api/manager-chat/wake`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ manager: 'hermes', repoId: 'stable-repo-id', instruction })
  });

  try {
    const [first, second] = await Promise.all([
      wake('Remember code word WAKE-QUEUE-819. Reply only OK.'),
      wake('What code word did I ask you to remember in the previous chat? Reply only with the code word.')
    ]);
    assert.equal(first.status, 202);
    assert.equal(second.status, 202);

    let replies: string[] = [];
    for (let attempt = 0; attempt < 200; attempt += 1) {
      replies = readLog(profile, { stateDir })
        .filter((event) => event.type === 'assistant/message')
        .map((event) => event.text);
      if (replies.length === 2 && updates.length === 4) break;
      await new Promise((resolveWait) => setTimeout(resolveWait, 25));
    }
    assert.deepEqual(replies, ['OK', 'WAKE-QUEUE-819']);
    assert.equal(updates.length, 4, 'each wake publishes both its start and terminal state');
  } finally {
    await new Promise<void>((done) => server.close(() => done()));
    setChatEventPublishers({});
    process.env = savedEnv;
    resetCachedCoordinatorIdentity();
    rmSync(root, { recursive: true, force: true });
  }
});
