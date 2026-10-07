import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import test from 'node:test';
import type { NodeObservationSnapshot, RunningWorker, StatusSnapshot } from '@git-agent-harness/contracts';
import { runningWorkers } from './runningWorkers.js';
const now = Date.parse('2026-10-06T12:00:00Z');
const worker = (run_id: string): RunningWorker => ({ work_id: '#1431', run_id, mode: 'improve', backend: 'codex-work', runner: 'codex', backend_instance: 'work-account', requested_model: 'requested', model: 'routed', actual_model: null, node_id: null, branch: 'gah/1431', started_at: new Date(now - 1000).toISOString(), last_activity_at: new Date(now - 1000).toISOString(), attempt: 2, stale_after_seconds: 900, state: 'running' });
const status = (workers?: RunningWorker[]) => ({ profile: { profile: 'gah' }, running_workers: workers, active_claims: [{ work_id: 'unrelated' }] } as StatusSnapshot);
const node = (id: string, workers: RunningWorker[], extra = {}) => ({ node_id: id, profile: 'gah', state: 'healthy', running_workers: workers, ...extra } as NodeObservationSnapshot);

test('fleet roster uses source-node routing facts and ignores unrelated profile, claims and duplicate coordinator', () => {
  const rows = runningWorkers(status([worker('local')]), [node('remote', [worker('remote')]), node('central', [worker('duplicate')]), node('other', [worker('wrong-profile')], { profile: 'other' })], 'central', now);
  assert.equal(rows.length, 2);
  assert.deepEqual(rows.map(row => row.node_id), ['central', 'remote']);
  assert.equal(rows[1].backend_instance, 'work-account');
  assert.equal(rows[1].model, 'routed');
  assert.equal(rows[1].requested_model, 'requested');
  assert.equal(rows[1].actual_model, null);
  assert.deepEqual(runningWorkers(status(), [], 'central', now), []);
});
test('idle and unreachable workers remain visible as stale; completed invocations disappear', () => {
  const idle = worker('idle'); idle.last_activity_at = new Date(now - 900_000).toISOString();
  const rows = runningWorkers(status([idle]), [node('remote', [worker('crashed')], { state: 'unreachable' })], 'central', now);
  assert.deepEqual(rows.map(row => row.state), ['stale', 'stale']);
  assert.deepEqual(runningWorkers(status([]), [node('remote', [])], 'central', now), []);
});

test('node observations preserve their source identity and retain crashed workers until a successful empty observation', async t => {
  const { RegistryService } = await import('./registryService.js');
  const { COORDINATOR_SCHEMA_DIGEST } = await import('./coordinatorIdentity.js');
  const { COORDINATOR_VERSION } = await import('@git-agent-harness/contracts');
  const root = mkdtempSync(join(tmpdir(), 'gah-worker-roster-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const registry = new RegistryService(join(root, 'registry.json'));
  registry.registerNode({ node_id: 'remote', display_name: 'Remote', advertised_url: 'http://127.0.0.1:3773', transport_mode: 'loopback', secret_ref: 'env:UNUSED_LOOPBACK_TOKEN', profiles: ['gah'], version: COORDINATOR_VERSION, schema_digest: COORDINATOR_SCHEMA_DIGEST });
  let offline = false;
  let workers = [worker('remote-run')];
  t.mock.method(globalThis, 'fetch', async () => {
    if (offline) throw new Error('Worker stopped');
    return Response.json({ ...status(workers), generated_at: new Date().toISOString(), nodes: [node('nested', [worker('must-not-relay')])] });
  });
  await registry.getNodeObservations();
  let observed = registry.getCachedObservations()[0];
  assert.equal(observed.running_workers?.length, 1);
  assert.equal(observed.running_workers?.[0].node_id, 'remote');
  offline = true;
  const failedObservations = await registry.getNodeObservations();
  observed = registry.getCachedObservations()[0];
  assert.equal(observed.state, 'unreachable');
  assert.equal(observed.running_workers?.[0].state, 'stale');
  assert.equal(observed.running_workers?.[0].model, 'routed');
  assert.equal(runningWorkers(status([]), registry.getCachedObservations(), 'central', now)[0]?.state, 'stale');
  assert.equal(runningWorkers(status([]), failedObservations, 'central', now)[0]?.state, 'stale');
  assert.deepEqual(runningWorkers({ ...status([]), profile: { profile: 'other' } } as StatusSnapshot, failedObservations, 'central', now), []);
  offline = false; workers = [];
  await registry.getNodeObservations();
  assert.deepEqual(registry.getCachedObservations()[0].running_workers, []);
});

test('fleet roster includes each declared profile and retains only failed profile observations', async t => {
  const { RegistryService } = await import('./registryService.js');
  const { COORDINATOR_SCHEMA_DIGEST } = await import('./coordinatorIdentity.js');
  const { COORDINATOR_VERSION } = await import('@git-agent-harness/contracts');
  const root = mkdtempSync(join(tmpdir(), 'gah-worker-profiles-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const registry = new RegistryService(join(root, 'registry.json'));
  registry.registerNode({ node_id: 'remote', display_name: 'Remote', advertised_url: 'http://127.0.0.1:3773', transport_mode: 'loopback', secret_ref: 'env:UNUSED_LOOPBACK_TOKEN', profiles: ['gah', 'other'], version: COORDINATOR_VERSION, schema_digest: COORDINATOR_SCHEMA_DIGEST });
  let failedProfile: string | null = null;
  let completed = false;
  t.mock.method(globalThis, 'fetch', async (url: string) => {
    const profile = new URL(url).searchParams.get('profile')!;
    if (profile === failedProfile) throw new Error('Profile unavailable');
    return Response.json({ ...status(completed ? [] : [worker(`${profile}-run`)]), profile: { profile }, generated_at: new Date().toISOString() });
  });
  const roster = (profile: string) => runningWorkers({ ...status([]), profile: { profile } } as StatusSnapshot, registry.getCachedObservations(), 'central', now);
  await registry.getNodeObservations();
  assert.deepEqual(roster('gah').map(row => row.run_id), ['gah-run']);
  assert.deepEqual(roster('other').map(row => row.run_id), ['other-run']);
  failedProfile = 'other';
  await registry.getNodeObservations();
  assert.equal(roster('gah')[0].state, 'running');
  assert.equal(roster('other')[0].state, 'stale');
  failedProfile = 'gah';
  await registry.getNodeObservations();
  assert.equal(roster('gah')[0].state, 'stale');
  assert.equal(roster('other')[0].state, 'running');
  failedProfile = null;
  completed = true;
  await registry.getNodeObservations();
  assert.deepEqual(roster('gah'), []);
  assert.deepEqual(roster('other'), []);
});
