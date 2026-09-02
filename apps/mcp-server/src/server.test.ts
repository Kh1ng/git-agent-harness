import assert from 'node:assert/strict';
import test from 'node:test';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { createGahMcpServer } from './server.js';

async function connectedPair() {
  const server = createGahMcpServer();
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await server.connect(serverTransport);
  const client = new Client({ name: 'gah-mcp-test', version: '0.0.0' });
  await client.connect(clientTransport);
  return { client, server };
}

test('lists usage and orchestration tools and forwards their HTTP calls', async () => {
  const originalFetch = globalThis.fetch;
  const requests: Array<{ url: string; init?: RequestInit & { dispatcher?: unknown } }> = [];
  globalThis.fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' }
    });
  };

  const { client, server } = await connectedPair();
  try {
    const tools = await client.listTools();
    const names = new Set(tools.tools.map((tool) => tool.name));
    for (const name of ['gah_info', 'gah_usage_rollup', 'gah_events', 'gah_controller_activity', 'gah_loop_status']) {
      assert(names.has(name), `missing ${name}`);
    }

    await client.callTool({
      name: 'gah_usage_rollup',
      arguments: { profile: 'gah', days: 30 }
    });
    assert.equal(requests.at(-1)?.url, 'http://127.0.0.1:3773/api/usage/rollup?profile=gah&days=30');

    await client.callTool({
      name: 'gah_dispatch',
      arguments: {
        profile: 'gah',
        providerKind: 'github',
        instanceId: 'local',
        repo: 'Kh1ng/git-agent-harness',
        mode: 'fix',
        mr: '1100',
        backend: 'claude',
        model: 'sonnet',
        retries: 1,
        dryRun: true
      }
    });
    const dispatch = requests.at(-1);
    assert.equal(dispatch?.url, 'http://127.0.0.1:3773/api/dispatch');
    assert(dispatch?.init?.dispatcher, 'terminal dispatch must override the five-minute fetch timeout');
    assert.deepEqual(JSON.parse(String(dispatch?.init?.body)), {
      profile: 'gah',
      providerKind: 'github',
      instanceId: 'local',
      repo: 'Kh1ng/git-agent-harness',
      mode: 'fix',
      mr: '1100',
      backend: 'claude',
      model: 'sonnet',
      retries: 1,
      dryRun: true,
      waitForCompletion: true,
      waitTimeoutSeconds: 3_600
    });
  } finally {
    await client.close();
    await server.close();
    globalThis.fetch = originalFetch;
  }
});
