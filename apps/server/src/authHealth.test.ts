import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { AuthProbe, NodeObservationSnapshot } from '@git-agent-harness/contracts';
import { AuthHealthMonitor, AuthHealthProber, parseNodeAuthHealth } from './authHealth.js';

type RawProbe = Omit<AuthProbe, 'since'>;
const copilot = (state: AuthProbe['state']): RawProbe => ({ backend: 'opencode', provider: 'github-copilot', state, source: 'probe' });

function fixture(local: RawProbe[] = [], classify = async () => true) {
  let report = { checked_at: '2026-09-30T12:00:00Z', probes: local };
  let clock = Date.parse('2026-09-30T12:00:00Z');
  const prober = new AuthHealthProber(async () => ({ ...report, checked_at: new Date(clock).toISOString() }));
  const workers: NodeObservationSnapshot[] = [];
  const monitor = new AuthHealthMonitor({ nodeId: 'central', nodeName: 'Central', prober }, () => workers, classify, () => clock);
  const events: string[] = [];
  monitor.onTransition(({ event }) => events.push(`${event.kind} ${event.title}`));
  return {
    prober, monitor, events, workers,
    setLocal: (probes: RawProbe[]) => { report = { ...report, probes }; },
    tick: (ms = 60_000) => { clock += ms; }
  };
}

test('a login that stops working is announced once, and again when it comes back (#1271)', async () => {
  const f = fixture([copilot('ok')]);
  await f.prober.refresh();
  assert.deepEqual(f.events, []);
  f.setLocal([copilot('expired')]);
  f.tick();
  await f.prober.refresh();
  f.tick();
  await f.prober.refresh();
  assert.deepEqual(f.events, ['auth_expired Central · opencode · github-copilot: login expired']);
  assert.deepEqual(f.monitor.rows().map((row) => [row.node_name, row.state]), [['Central', 'expired']]);
  f.setLocal([copilot('ok')]);
  await f.prober.refresh();
  assert.deepEqual(f.events.at(-1), 'auth_restored Central · opencode · github-copilot: login restored');
  assert.equal(f.events.length, 2);
});

test('a worker report is read from its observation, and a never-logged-in CLI is shown but not pushed', async () => {
  const f = fixture();
  const observation = (probes: AuthProbe[]) => ({ node_id: 'mac', display_name: 'Mac', auth_health: { checked_at: 'x', probes } }) as unknown as NodeObservationSnapshot;
  f.workers.push(observation([{ backend: 'hermes', provider: null, state: 'missing', source: 'probe', since: 'a' }]));
  f.monitor.observationsChanged();
  assert.deepEqual(f.events, []);
  assert.equal(f.monitor.rows()[0].state, 'missing');
  f.workers[0] = observation([{ ...copilot('expired'), since: 'b' }]);
  f.monitor.observationsChanged();
  assert.deepEqual(f.events, ['auth_expired Mac · opencode · github-copilot: login expired']);
});

test('a chat turn that fails with 401 is expired at once; the next good turn restores it', async () => {
  const f = fixture([{ backend: 'claude', provider: null, state: 'ok', source: 'probe' }]);
  await f.prober.refresh();
  await f.monitor.turnFinished('mac', 'claude', 'API Error: 401 Unauthorized');
  assert.deepEqual(f.events, ['auth_expired mac · claude: login expired']);
  assert.equal(f.monitor.rows().find((row) => row.node_id === 'mac')?.source, 'chat');
  await f.monitor.turnFinished('mac', 'claude', 'API Error: 401 Unauthorized');
  assert.equal(f.events.length, 1, 'a repeated failure is not a new transition');
  await f.monitor.turnFinished('mac', 'claude');
  assert.equal(f.monitor.rows().some((row) => row.node_id === 'mac'), false);
  assert.deepEqual(f.events, ['auth_expired mac · claude: login expired', 'auth_restored mac · claude: login restored']);
});

test('a worker model catalog becoming unknown neither expires its login nor changes healthy Codex', () => {
  const f = fixture();
  const codex: AuthProbe = { backend: 'codex', provider: null, state: 'ok', source: 'probe' };
  const observation = (state: AuthProbe['state']) => ({
    node_id: 'mac', display_name: 'Mac',
    auth_health: { checked_at: 'now', probes: [{ ...copilot(state), detail: 'No models were listed for this provider. Login validity is unknown.' }, codex] }
  }) as unknown as NodeObservationSnapshot;
  f.workers.push(observation('ok'));
  f.monitor.observationsChanged();
  f.workers[0] = observation('unknown');
  f.monitor.observationsChanged();
  assert.deepEqual(f.monitor.rows().map(row => [row.backend, row.provider, row.state]), [
    ['opencode', 'github-copilot', 'unknown'], ['codex', null, 'ok']
  ]);
  assert.deepEqual(f.events, [], 'model discovery is not an expired-login attention event');
});

test('a failure that is not about authentication changes nothing', async () => {
  const f = fixture([], async () => false);
  await f.monitor.turnFinished('central', 'codex', 'the model is overloaded');
  assert.deepEqual([f.events, f.monitor.rows()], [[], []]);
});

test('worker reports are validated before central trusts them', () => {
  assert.equal(parseNodeAuthHealth(null), null);
  assert.equal(parseNodeAuthHealth({ checked_at: 1, probes: [] }), null);
  const parsed = parseNodeAuthHealth({ checked_at: 'now', probes: [
    { backend: 'codex', provider: null, state: 'ok', source: 'probe', token: 'secret' },
    { backend: 'codex', provider: null, state: 'sideways', source: 'probe' },
    { backend: 'x'.repeat(65), provider: null, state: 'ok', source: 'probe' }
  ] });
  assert.deepEqual(parsed, { checked_at: 'now', probes: [{ backend: 'codex', provider: null, state: 'ok', source: 'probe' }] });
});
