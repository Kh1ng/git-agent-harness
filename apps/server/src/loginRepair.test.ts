import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { LoginRepairView } from '@git-agent-harness/contracts';
import { LoginRepairBroker, LoginRepairError, LoginRepairs, loadProviderKeys, parseLoginRepairView } from './loginRepair.js';
import type { RegistryService } from './registryService.js';

function sandbox() {
  const root = mkdtempSync(join(tmpdir(), 'gah-login-repair-'));
  const bin = join(root, 'bin');
  const env: NodeJS.ProcessEnv = { PATH: `${bin}:/usr/bin:/bin`, HOME: root };
  const tool = (name: string, script: string) => {
    mkdirSync(bin, { recursive: true });
    writeFileSync(join(bin, name), `#!/bin/sh\n${script}\n`);
    chmodSync(join(bin, name), 0o755);
  };
  return { root, env, tool, done: () => rmSync(root, { recursive: true, force: true }) };
}

async function until(read: () => LoginRepairView | null, status: LoginRepairView['status'], ms = 5_000): Promise<LoginRepairView> {
  const deadline = Date.now() + ms;
  for (;;) {
    const view = read();
    if (view?.status === status) return view;
    if (Date.now() > deadline) throw new Error(`still ${view?.status ?? 'missing'}; wanted ${status}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

test('codex device auth shows only the link and code, then re-checks the login (#1272)', async () => {
  const box = sandbox();
  try {
    box.tool('codex', `cat <<'OUT'
Follow these steps to sign in with ChatGPT using device code authorization:
1. Open this link in your browser and sign in to your account
   https://auth.openai.com/codex/device
2. Enter this one-time code (expires in 15 minutes)
   K7QD-M2XPA
debug: token=secret-provider-output
OUT
sleep 0.3`);
    let rechecked = 0;
    const repairs = new LoginRepairs({ env: box.env, onSuccess: async () => { rechecked++; } });
    const started = repairs.start({ backend: 'codex', provider: null });
    const open = await until(() => repairs.view(started.id), 'open_url');
    assert.deepEqual([open.status === 'open_url' && open.url, open.status === 'open_url' && open.code], ['https://auth.openai.com/codex/device', 'K7QD-M2XPA']);
    assert.doesNotMatch(JSON.stringify(open), /secret-provider-output/);
    await until(() => repairs.view(started.id), 'succeeded');
    assert.equal(rechecked, 1);
  } finally { box.done(); }
});

test('a link to an unexpected host is never shown', async () => {
  const box = sandbox();
  try {
    box.tool('codex', `echo 'Open https://evil.example/codex/device and enter ABCD-EFGH'; sleep 0.2; exit 1`);
    const repairs = new LoginRepairs({ env: box.env });
    const { id } = repairs.start({ backend: 'codex', provider: null });
    const failed = await until(() => repairs.view(id), 'failed');
    assert.doesNotMatch(JSON.stringify(failed), /evil\.example/);
  } finally { box.done(); }
});

test('claude pastes the code back into its own login (#1272)', async () => {
  const box = sandbox();
  try {
    box.tool('claude', `echo "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&state=x"
printf 'Paste code here if prompted > '
read code
[ "$code" = "good-code#state" ]`);
    const repairs = new LoginRepairs({ env: box.env });
    const { id } = repairs.start({ backend: 'claude', provider: null });
    const waiting = await until(() => repairs.view(id), 'awaiting_input');
    assert.equal(waiting.status === 'awaiting_input' && waiting.secret, false);
    await assert.rejects(repairs.submit(id, 'two\nlines'), (error: unknown) => error instanceof LoginRepairError && error.status === 400);
    await repairs.submit(id, 'good-code#state');
    await until(() => repairs.view(id), 'succeeded');
  } finally { box.done(); }
});

test('a login that runs past its deadline is stopped and expired', async () => {
  const box = sandbox();
  try {
    box.tool('codex', `echo 'https://auth.openai.com/codex/device'; sleep 30`);
    const repairs = new LoginRepairs({ env: box.env, ttlMs: 300 });
    const { id } = repairs.start({ backend: 'codex', provider: null });
    await until(() => repairs.view(id), 'expired');
  } finally { box.done(); }
});

function githubFetch(outcomes: Record<string, unknown>[]) {
  const calls: string[] = [];
  const fake = (async (url: string | URL) => {
    calls.push(String(url));
    const body = String(url).endsWith('/login/device/code')
      ? { device_code: 'dev-123', user_code: 'WDJB-MJHT', verification_uri: 'https://github.com/login/device', interval: 0 }
      : outcomes.shift() ?? { error: 'expired_token' };
    return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { fake, calls };
}

test('Copilot runs GitHub device flow and stores the token the way opencode does (#1272)', async () => {
  const box = sandbox();
  const authPath = join(box.root, 'opencode/auth.json');
  try {
    mkdirSync(join(box.root, 'opencode'), { recursive: true });
    writeFileSync(authPath, JSON.stringify({ anthropic: { type: 'api', key: 'keep-me' } }));
    const { fake } = githubFetch([{ error: 'authorization_pending' }, { access_token: 'gho_new' }]);
    const repairs = new LoginRepairs({ env: box.env, fetch: fake, opencodeAuthPath: authPath, minPollMs: 100 });
    const { id } = repairs.start({ backend: 'opencode', provider: 'github-copilot' });
    const open = await until(() => repairs.view(id), 'open_url');
    assert.equal(open.status === 'open_url' && open.code, 'WDJB-MJHT');
    await until(() => repairs.view(id), 'succeeded');
    assert.deepEqual(JSON.parse(readFileSync(authPath, 'utf8')), {
      anthropic: { type: 'api', key: 'keep-me' },
      'github-copilot': { type: 'oauth', refresh: 'gho_new', access: '', expires: 0 }
    });
    assert.equal(statSync(authPath).mode & 0o777, 0o600);
  } finally { box.done(); }
});

test('an unreadable opencode auth.json is never overwritten', async () => {
  const box = sandbox();
  const authPath = join(box.root, 'auth.json');
  try {
    writeFileSync(authPath, '{not json');
    const { fake } = githubFetch([{ access_token: 'gho_new' }]);
    const repairs = new LoginRepairs({ env: box.env, fetch: fake, opencodeAuthPath: authPath, minPollMs: 1 });
    const { id } = repairs.start({ backend: 'opencode', provider: 'github-copilot' });
    const failed = await until(() => repairs.view(id), 'failed');
    assert.match(failed.status === 'failed' ? failed.reason : '', /unreadable/);
    assert.equal(readFileSync(authPath, 'utf8'), '{not json');
  } finally { box.done(); }
});

test('missing repository CLIs stop before authorization and link to official installation', async () => {
  const box = sandbox();
  box.env.PATH = join(box.root, 'empty');
  try {
    let requests = 0;
    const repairs = new LoginRepairs({ env: box.env, fetch: async () => { requests++; throw new Error('unexpected OAuth request'); } });
    for (const [backend, provider, guide] of [
      ['gh', 'github', 'https://cli.github.com/'],
      ['glab', 'gitlab', 'https://gitlab.com/gitlab-org/cli#installation'],
    ]) {
      const { id } = repairs.start({ backend, provider });
      const view = await until(() => repairs.view(id), 'install_required');
      assert.equal(view.status === 'install_required' && view.install_url, guide);
      assert.equal(parseLoginRepairView(view)?.status, 'install_required');
      assert.equal(parseLoginRepairView({ ...view, install_url: 'https://evil.example/' }), null);
    }
    assert.equal(requests, 0);
  } finally { box.done(); }
});

test('gh receives the device-flow token on stdin, never argv', async () => {
  const box = sandbox();
  try {
    box.tool('gh', `printf '%s\\n' "$@" > "$HOME/argv"; cat > "$HOME/stdin"`);
    const { fake } = githubFetch([{ access_token: 'gho_gh' }]);
    const repairs = new LoginRepairs({ env: box.env, fetch: fake, minPollMs: 1 });
    const { id } = repairs.start({ backend: 'gh', provider: 'github' });
    await until(() => repairs.view(id), 'succeeded');
    assert.equal(readFileSync(join(box.root, 'argv'), 'utf8'), 'auth\nlogin\n--hostname\ngithub.com\n--with-token\n');
    assert.equal(readFileSync(join(box.root, 'stdin'), 'utf8'), 'gho_gh\n');
  } finally { box.done(); }
});

test('pasted API keys go to the node store: opencode auth.json or the private env file', async () => {
  const box = sandbox();
  const authPath = join(box.root, 'auth.json');
  const keysPath = join(box.root, 'provider-keys.env');
  try {
    const repairs = new LoginRepairs({ env: box.env, opencodeAuthPath: authPath, providerKeysPath: keysPath });
    const opencode = repairs.start({ backend: 'opencode', provider: 'anthropic' });
    assert.equal(opencode.status === 'awaiting_input' && opencode.secret, true);
    await assert.rejects(repairs.submit(opencode.id, 'has space'), LoginRepairError);
    await repairs.submit(opencode.id, 'sk-ant-new');
    await until(() => repairs.view(opencode.id), 'succeeded');
    assert.deepEqual(JSON.parse(readFileSync(authPath, 'utf8')), { anthropic: { type: 'api', key: 'sk-ant-new' } });

    const mistral = repairs.start({ backend: 'api', provider: 'mistral' });
    await repairs.submit(mistral.id, 'mistral-new');
    assert.equal(box.env.MISTRAL_API_KEY, 'mistral-new');
    assert.equal(statSync(keysPath).mode & 0o777, 0o600);
    const reloaded: NodeJS.ProcessEnv = {};
    loadProviderKeys(keysPath, reloaded);
    assert.deepEqual(reloaded, { MISTRAL_API_KEY: 'mistral-new' });
    assert.doesNotMatch(JSON.stringify(repairs.view(mistral.id)), /mistral-new/);
  } finally { box.done(); }
});

test('a login only a terminal can fix says so', () => {
  const repairs = new LoginRepairs();
  const view = repairs.start({ backend: 'hermes', provider: null });
  assert.equal(view.status, 'manual');
});

test('only the device that started a repair can read its code (#1272)', async () => {
  const box = sandbox();
  try {
    box.tool('codex', `echo 'Enter this one-time code'; echo 'https://auth.openai.com/codex/device ABCD-EFGHJ'; sleep 5`);
    const local = new LoginRepairs({ env: box.env });
    const broker = new LoginRepairBroker({ localNodeId: 'central', local, registry: {} as RegistryService });
    const phone = { kind: 'device' as const, id: 'phone' };
    const { key, repair } = await broker.start(phone, 'central', { backend: 'codex', provider: null });
    const mine = await broker.view(repair.id, phone, key);
    assert.equal(mine.node_id, 'central');
    for (const [who, secret] of [[{ kind: 'device' as const, id: 'tablet' }, key], [{ kind: 'owner' as const }, key], [phone, 'f'.repeat(64)]] as const) {
      await assert.rejects(broker.view(repair.id, who, secret), (error: unknown) => error instanceof LoginRepairError && error.status === 404);
    }
    await broker.cancel(repair.id, phone, key);
    assert.equal((await broker.view(repair.id, phone, key)).status, 'failed');
  } finally { box.done(); }
});

test('central relays a worker repair and rejects a worker view with a non-HTTPS link', async () => {
  const worker = (view: Record<string, unknown>) => (async (_url: string | URL, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body));
    return new Response(JSON.stringify(body.action === 'start' || body.action === 'status' ? view : {}), { status: body.action === 'start' ? 201 : 200 });
  }) as typeof fetch;
  const registry = { getNode: () => ({ node_id: 'mac', advertised_url: 'http://127.0.0.1:9', secret_ref: 'env:WORKER_TOKEN' }) } as unknown as RegistryService;
  process.env.WORKER_TOKEN = 'worker-token';
  const base = { id: 'remote-1', node_id: '', backend: 'opencode', provider: 'github-copilot', expires_at: new Date(Date.now() + 60_000).toISOString() };
  let refreshed = '';
  const good = new LoginRepairBroker({ localNodeId: 'central', local: new LoginRepairs(), registry, fetch: worker({ ...base, status: 'succeeded' }), onRemoteSuccess: (nodeId) => { refreshed = nodeId; } });
  const phone = { kind: 'device' as const, id: 'phone' };
  const { key, repair } = await good.start(phone, 'mac', { backend: 'opencode', provider: 'github-copilot' });
  assert.equal((await good.view(repair.id, phone, key)).status, 'succeeded');
  assert.equal(refreshed, 'mac', 'a worker success pulls its fresh login report');

  const evil = new LoginRepairBroker({ localNodeId: 'central', local: new LoginRepairs(), registry, fetch: worker({ ...base, status: 'open_url', url: 'http://phish.example/', code: 'ABCD-EFGH' }) });
  await assert.rejects(evil.start(phone, 'mac', { backend: 'opencode', provider: 'github-copilot' }), (error: unknown) => error instanceof LoginRepairError && error.status === 502);
  assert.equal(parseLoginRepairView({ ...base, status: 'open_url', url: 'https://github.com/login/device', code: 'ABCD-EFGH', extra: 'dropped' })?.status, 'open_url');
  delete process.env.WORKER_TOKEN;
});

test('the provider-key file is ignored for variables GAH does not manage', () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-provider-keys-'));
  try {
    const path = join(root, 'keys.env');
    writeFileSync(path, 'PATH=/evil\nNOUS_API_KEY=nous-key\n');
    const env: NodeJS.ProcessEnv = {};
    loadProviderKeys(path, env);
    assert.deepEqual(env, { NOUS_API_KEY: 'nous-key' });
    assert.equal(existsSync(join(root, 'missing')), false);
    loadProviderKeys(join(root, 'missing'), env);
    assert.deepEqual(env, { NOUS_API_KEY: 'nous-key' }, 'a missing file changes nothing');
  } finally { rmSync(root, { recursive: true, force: true }); }
});
