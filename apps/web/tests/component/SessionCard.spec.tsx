import { expect, test } from '@playwright/experimental-ct-react';
import type { Session } from '@git-agent-harness/contracts';
import { SessionCard } from '../../src/components/SessionCard.js';

const session: Session = {
  id: 'session-123',
  providerKind: 'claude',
  instanceId: 'claude-0',
  status: 'running',
  repo: 'Kh1ng/git-agent-harness',
  mode: 'improve',
  target: '#1112',
  backend: 'claude'
};

test('names a dispatch by its work instead of its repository', async ({ mount }) => {
  const component = await mount(<SessionCard session={session} onClick={() => undefined} />);

  await expect(component.getByRole('heading', { name: 'Improve #1112' })).toBeVisible();
  await expect(component.getByText('Kh1ng/git-agent-harness', { exact: true })).toBeVisible();
});
