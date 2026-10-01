import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, rmSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import express from 'express';
import type { ChatSessionSummary, PlanningEpicList, PlanningMap } from '@git-agent-harness/contracts';
import { grillPrompt, planningRouter, ticketPrompt, validAnswersPath, type PlanningDeps } from './planning.js';

const map: PlanningMap = {
  epic: 1,
  nodes: [
    { number: 1, title: 'Epic', url: 'https://g/1', labels: ['epic'], state: 'parent', depth: 0, waiting_on: [] },
    { number: 2, title: 'Schema', url: 'https://g/2', labels: [], state: 'done', depth: 1, waiting_on: [] },
    { number: 3, title: 'API', url: 'https://g/3', labels: [], state: 'ready', depth: 1, waiting_on: [] },
    { number: 4, title: 'UI', url: 'https://g/4', labels: [], state: 'blocked', depth: 1, waiting_on: [3] }
  ],
  edges: [
    { from: 1, to: 2, kind: 'child' }, { from: 1, to: 3, kind: 'child' }, { from: 1, to: 4, kind: 'child' },
    { from: 2, to: 3, kind: 'blocks' }, { from: 3, to: 4, kind: 'blocks' }
  ],
  frontier: [3],
  missing: []
};

interface Started { profile: string; seed: { title: string; text: string; worktree: boolean }; backend?: string }

async function withRouter(run: (base: string, calls: { maps: string[]; started: Started[] }) => Promise<void>): Promise<void> {
  const directory = mkdtempSync(join(tmpdir(), 'gah-planning-'));
  const calls = { maps: [] as string[], started: [] as Started[] };
  const deps = {
    map: async (profile: string, epic?: number) => {
      calls.maps.push(`${profile}#${epic ?? ''}`);
      if (profile === 'broken') throw new Error('gh failed at /home/secret');
      return epic === undefined ? { epics: [] } satisfies PlanningEpicList : map;
    },
    startChat: async (profile: string, seed: Started['seed'], backend?: string) => {
      calls.started.push({ profile, seed, backend });
      return { id: 'session-1' } as ChatSessionSummary;
    },
    settingsPath: join(directory, 'planning.json')
  } as PlanningDeps;
  const app = express();
  app.use(express.json());
  app.use('/api/planning', planningRouter(deps));
  const server = http.createServer(app);
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    await run(`http://127.0.0.1:${(server.address() as AddressInfo).port}/api/planning`, calls);
  } finally {
    server.close();
    rmSync(directory, { recursive: true, force: true });
  }
}

const send = (url: string, method: string, body: unknown) =>
  fetch(url, { method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });

test('maps are cached briefly, refreshable, and provider failures do not leak output', async () => {
  await withRouter(async (base, calls) => {
    assert.equal((await fetch(`${base}/map?profile=demo&epic=1`)).status, 200);
    assert.equal((await fetch(`${base}/map?profile=demo&epic=1`)).status, 200);
    assert.deepEqual(calls.maps, ['demo#1']);
    await fetch(`${base}/map?profile=demo&epic=1&refresh=1`);
    assert.deepEqual(calls.maps, ['demo#1', 'demo#1']);
    assert.deepEqual(await (await fetch(`${base}/epics?profile=demo`)).json(), { epics: [] });
    const broken = await fetch(`${base}/map?profile=broken&epic=1`);
    assert.equal(broken.status, 502);
    assert.doesNotMatch(await broken.text(), /secret/);
    for (const query of ['profile=demo', 'profile=demo&epic=0', 'profile=demo&epic=1x', 'epic=1']) {
      assert.equal((await fetch(`${base}/map?${query}`)).status, 400, query);
    }
  });
});

test('settings default to issues, persist privately, and refuse paths outside the checkout', async () => {
  await withRouter(async (base) => {
    assert.deepEqual(await (await fetch(`${base}/settings?profile=demo`)).json(), { answers: 'issues', path: 'docs/PLANNING.md' });
    const saved = await send(`${base}/settings`, 'PUT', { profile: 'demo', answers: 'file', path: 'plans/roadmap.md' });
    assert.equal(saved.status, 200);
    assert.deepEqual(await (await fetch(`${base}/settings?profile=demo`)).json(), { answers: 'file', path: 'plans/roadmap.md' });
    assert.deepEqual(await (await fetch(`${base}/settings?profile=other`)).json(), { answers: 'issues', path: 'docs/PLANNING.md' });
    for (const path of ['../escape.md', '/etc/x.md', 'notes.txt', 'a//b.md', 'a/./b.md', 'a\\b.md']) {
      assert.equal(validAnswersPath(path), false, path);
      assert.equal((await send(`${base}/settings`, 'PUT', { profile: 'demo', answers: 'file', path })).status, 400, path);
    }
  });
  const directory = mkdtempSync(join(tmpdir(), 'gah-planning-mode-'));
  try {
    const path = join(directory, 'planning.json');
    const app = express().use(express.json()).use(planningRouter({ map: async () => map, startChat: async () => ({}) as ChatSessionSummary, settingsPath: path } as unknown as PlanningDeps));
    const server = http.createServer(app);
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    await send(`http://127.0.0.1:${(server.address() as AddressInfo).port}/settings`, 'PUT', { profile: 'demo', answers: 'issues', path: 'docs/PLANNING.md' });
    server.close();
    assert.equal(statSync(path).mode & 0o777, 0o600);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('grill-me records answers where the project chose, and only a file needs a worktree', async () => {
  await withRouter(async (base, calls) => {
    assert.equal((await send(`${base}/chats`, 'POST', { profile: 'demo', kind: 'grill', epic: 1, idea: 'Offline mode' })).status, 201);
    const issues = calls.started[0].seed;
    assert.equal(issues.worktree, false);
    assert.equal(issues.title, 'Plan: #1');
    assert.match(issues.text, /The idea to plan: Offline mode/);
    assert.match(issues.text, /#4 UI: blocked, waiting on #3/);
    assert.match(issues.text, /`Parent: #1`/);
    assert.match(issues.text, /wait for my yes before filing/);

    await send(`${base}/settings`, 'PUT', { profile: 'demo', answers: 'file', path: 'plans/offline.md' });
    await send(`${base}/chats`, 'POST', { profile: 'demo', kind: 'grill', backend: 'codex' });
    const file = calls.started[1];
    assert.equal(file.seed.worktree, true);
    assert.equal(file.backend, 'codex');
    assert.match(file.seed.text, /`plans\/offline\.md`/);
    assert.match(file.seed.text, /Do not file issues/);
    assert.match(file.seed.text, /Start by asking me what I want to plan/);

    for (const body of [
      { profile: 'demo', kind: 'ticket', epic: 1 },
      { profile: 'demo', kind: 'other' },
      { profile: 'demo', kind: 'grill', backend: 'rm -rf' },
      { profile: 'demo', kind: 'grill', idea: 'x'.repeat(4001) }
    ]) {
      assert.equal((await send(`${base}/chats`, 'POST', body)).status, 400, JSON.stringify(body).slice(0, 60));
    }
  });
});

test('a ticket chat carries its blockers and the map, and changes nothing', async () => {
  await withRouter(async (base, calls) => {
    assert.equal((await send(`${base}/chats`, 'POST', { profile: 'demo', kind: 'ticket', epic: 1, ticket: 4 })).status, 201);
    const seed = calls.started[0].seed;
    assert.equal(seed.worktree, false);
    assert.equal(seed.title, 'Ticket #4');
    assert.match(seed.text, /^Ticket #4 UI \(https:\/\/g\/4\)/);
    assert.match(seed.text, /Its blockers[^\n]*\n- #3 API: ready/);
    assert.match(seed.text, /Frontier \(can start now\): #3/);
    assert.match(seed.text, /do not change files or the issue/);
  });
});

test('planning prompts use no em dashes', () => {
  const settings = { answers: 'issues' as const, path: 'docs/PLANNING.md' };
  for (const text of [grillPrompt({ map, idea: 'x', settings }), grillPrompt({ map: null, idea: '', settings: { ...settings, answers: 'file' } }), ticketPrompt(map, 4)]) {
    assert.doesNotMatch(text, /—/);
  }
});
