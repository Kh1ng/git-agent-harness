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
  let responseStatus = 200;
  globalThis.fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    return new Response(JSON.stringify(responseStatus === 200 ? { ok: true } : { message: 'Operation already accepted. Refresh status.' }), {
      status: responseStatus,
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
    // Issue #525: paid-route approval tools are exposed.
    for (const name of ['gah_route_approvals', 'gah_route_approval_grant', 'gah_route_approval_revoke']) {
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

    await client.callTool({
      name: 'gah_ledger_clear_attempts',
      arguments: { profile: 'gah', work_id: '519', dry_run: true }
    });
    assert.deepEqual(JSON.parse(String(requests.at(-1)?.init?.body)), {
      profile: 'gah',
      workId: '519',
      dryRun: true
    });

    for (let i = 0; i < 2; i++) {
      await client.callTool({ name: 'gah_hold_set', arguments: { profile: 'gah', work_id: '532' } });
    }
    assert.deepEqual(JSON.parse(String(requests.at(-1)?.init?.body)), {
      profile: 'gah',
      workId: '532'
    });
    const mutations = requests.filter(request => request.init?.method === 'POST');
    const keys = mutations.map(request => new Headers(request.init?.headers).get('Idempotency-Key'));
    for (const key of keys) assert.match(key ?? '', /^[A-Za-z0-9_-]{16,128}$/, 'Every MCP mutation supplies a valid key');
    assert.equal(new Set(keys).size, mutations.length, 'Separate tool calls use separate operation keys');
    assert.equal(new Headers(requests[0].init?.headers).has('Idempotency-Key'), false, 'Reads do not reserve mutation keys');

    responseStatus = 409;
    const beforeConflict = requests.length;
    const conflict = await client.callTool({ name: 'gah_hold_clear', arguments: { profile: 'gah', work_id: '532' } });
    assert.equal(conflict.isError, true);
    assert.match(JSON.stringify(conflict), /Refresh status/);
    assert.equal(requests.length, beforeConflict + 1, 'A duplicate conflict reaches the caller without an automatic retry');
  } finally {
    await client.close();
    await server.close();
    globalThis.fetch = originalFetch;
  }
});

test('paid-route approval tools hit the exact-scope mutation API', async () => {
  const originalFetch = globalThis.fetch;
  const requests: Array<{ url: string; init?: RequestInit & { dispatcher?: unknown } }> = [];
  globalThis.fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    const isList = String(input).includes('/api/route-approvals?');
    return new Response(
      JSON.stringify(
        isList
          ? [{ profile: 'real', work_id: '#653', backend: 'opencode', backend_instance: 'paid-a', model: 'provider/model', requested: true, approved: false }]
          : { ok: true }
      ),
      { status: 200, headers: { 'content-type': 'application/json' } }
    );
  };

  const { client, server } = await connectedPair();
  try {
    const listed = await client.callTool({ name: 'gah_route_approvals', arguments: { profile: 'real' } });
    assert.ok(String(requests.at(-1)?.url).includes('/api/route-approvals?profile=real'));
    assert.ok(JSON.stringify(listed).includes('#653'));

    await client.callTool({
      name: 'gah_route_approval_grant',
      arguments: { profile: 'real', work_id: '#653', backend: 'opencode', backend_instance: 'paid-a', model: 'provider/model' }
    });
    const grant = requests.at(-1);
    assert.ok(grant?.url.endsWith('/api/route-approvals/grant'));
    const body = JSON.parse(String(grant?.init?.body));
    assert.equal(body.confirm, true);
    assert.equal(body.work_id, '#653');
    assert.equal(body.backend_instance, 'paid-a');

    await client.callTool({
      name: 'gah_route_approval_revoke',
      arguments: { profile: 'real', work_id: '#653', backend: 'opencode' }
    });
    assert.ok(requests.at(-1)?.url.endsWith('/api/route-approvals/revoke'));
    const revokeBody = JSON.parse(String(requests.at(-1)?.init?.body));
    assert.equal(revokeBody.backend_instance, null);
    assert.equal(revokeBody.model, null);
  } finally {
    globalThis.fetch = originalFetch;
    await server.close();
  }
});
