import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import type { ChatSessionSummary, SkillBindingSummary } from '@git-agent-harness/contracts';
import { ChatSessionDetailDrawer } from '../../src/components/ChatSessionDetailDrawer.js';

const session: ChatSessionSummary = {
  id: 'chat-42', profile: 'fixture', worktreePath: '/tmp/chat-42', branch: 'gah/issue/fixture-42',
  backend: 'codex', backendInstance: 'codex-work', model: 'gpt-6-sol', reasoningEffort: 'high',
  title: '#42 Fix retries', createdAt: 1, lastActiveAt: 2, archivedAt: null, outcome: 'live', settledAt: null, settledReason: null
};
const binding: SkillBindingSummary = {
  profile: 'fixture', backend: 'codex', instance: 'codex-work', sessionId: 'chat-42', source: 'session', supported: true,
  selectedIds: ['review'], observedSkills: [{ id: 'review', version: '1' }],
  skills: [
    { id: 'review', version: '1', displayName: 'Review', description: 'Review changes', backends: ['codex'], source: 'test', bound: true },
    { id: 'tests', version: '1', displayName: 'Tests', description: 'Run focused tests', backends: ['codex'], source: 'test', bound: false }
  ]
};

test('consolidates session identity, usage, work, rename, skills, and archive controls', async ({ mount, page }, testInfo) => {
  const actions: string[] = [];
  const component = await mount(<ChatSessionDetailDrawer session={session} turnBusy={false} providerPicker={null}
    skillBinding={binding} skillBusy={false} usage={{ turns: 2, inputTokens: 1200, outputTokens: 300, totalTokens: 1500, estimatedCostUsd: 0.125, costIncomplete: false }}
    workId="#42" onRename={async (title) => { actions.push(`rename:${title}`); }} onArchive={async () => { actions.push('archive'); }}
    onRestore={async () => { actions.push('restore'); }} onToggleSkill={async (id) => { actions.push(`skill:${id}`); }}
    onInheritSkills={async () => { actions.push('inherit'); }} onOpenWork={(id) => actions.push(`work:${id}`)} onClose={() => actions.push('close')} />);

  await expect(component.getByText('codex-work')).toBeVisible();
  await expect(component.getByText('gpt-6-sol')).toBeVisible();
  await expect(component.getByText('1,500 total reported tokens')).toBeVisible();
  await expect(component.getByText('$0.1250')).toBeVisible();
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.screenshot({ path: testInfo.outputPath('session-detail-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await component.getByRole('button', { name: 'Open #42 and attempt history' }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('session-detail-mobile.png') });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

  await component.getByLabel('Session name').fill('Retry controller');
  await component.getByRole('button', { name: 'Save' }).click();
  await component.getByText('Tests', { exact: true }).click();
  await component.getByRole('button', { name: 'Use project default' }).click();
  await component.getByRole('button', { name: 'Open #42 and attempt history' }).click();
  expect(actions).toEqual(['rename:Retry controller', 'skill:tests', 'inherit', 'work:#42', 'close']);

  page.once('dialog', (dialog) => dialog.accept());
  await component.getByRole('button', { name: 'Archive session' }).click();
  expect(actions.at(-1)).toBe('archive');
});

test('restores archived sessions and keeps settled sessions terminal', async ({ mount }) => {
  let restored = 0;
  const archived = { ...session, worktreePath: null, archivedAt: 10, outcome: 'archived' as const };
  const component = await mount(<ChatSessionDetailDrawer session={archived} turnBusy={false} providerPicker={null}
    skillBinding={binding} skillBusy={false} usage={{ turns: 0, inputTokens: null, outputTokens: null, totalTokens: null, estimatedCostUsd: null, costIncomplete: false }}
    workId={null} onRename={async () => {}} onArchive={async () => {}} onRestore={async () => { restored += 1; }}
    onToggleSkill={async () => {}} onInheritSkills={async () => {}} onClose={() => {}} />);
  await component.getByRole('button', { name: 'Restore session' }).click();
  expect(restored).toBe(1);
  await component.update(<ChatSessionDetailDrawer session={{ ...archived, outcome: 'settled', settledReason: 'merged' }} turnBusy={false} providerPicker={null}
    skillBinding={binding} skillBusy={false} usage={{ turns: 0, inputTokens: null, outputTokens: null, totalTokens: null, estimatedCostUsd: null, costIncomplete: false }}
    workId={null} onRename={async () => {}} onArchive={async () => {}} onRestore={async () => { restored += 1; }}
    onToggleSkill={async () => {}} onInheritSkills={async () => {}} onClose={() => {}} />);
  await expect(component.getByText('Settled sessions are terminal and cannot be restored.')).toBeVisible();
});
