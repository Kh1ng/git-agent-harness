/**
 * Issue → chat sessions (issue-to-workflow): grab a provider issue, branch
 * for it, mark it in progress, and open a GAH chat seeded with the issue.
 *
 * The session rides the existing chat-session machinery (worktree, cwd
 * binding, resume, archive) with only two additions: the branch is named
 * for the issue (`gah/issue/<repo_id>-<n>`) so its fate is readable in the
 * provider UI too, and the session log is seeded with the issue as the
 * opening message — durable in the transcript and replayed into every
 * backend's context, so the session is model-interchangeable from turn one.
 *
 * Starting is idempotent per (profile, issue): a live session on the
 * issue's branch is returned as-is instead of creating a second one.
 */

import type { ChatIssueSummary, ChatSessionSummary, ProfileSummary } from '@git-agent-harness/contracts';
import { AsyncTtlCache } from '../asyncTtlCache.js';
import { type ForgeProject, execProjectCli, execProviderCli } from './providerCli.js';
import { seedOpeningMessage } from './sessionLog.js';
import {
  chatSessionStoreOptions,
  createSession,
  listSessions,
  type ChatSessionStoreOptions
} from './chatSessions.js';

const ISSUES_CACHE_TTL_MS = 15_000;
const issuesCache = new AsyncTtlCache<string, ChatIssueSummary[]>(ISSUES_CACHE_TTL_MS);

interface ProviderIssue {
  number?: number;
  iid?: number;
  title: string;
  body?: string | null;
  description?: string | null;
  state: string;
  url?: string | null;
  web_url?: string | null;
  labels: { name?: string; name_and_description?: string }[] | string[] | null;
  updatedAt?: string | null;
  updated_at?: string | null;
}

/** Default label marking an issue somebody is working on via chat. Added
 * only when the label exists at the provider (creating labels silently on
 * someone's repo is not ours to do). */
export const ISSUE_IN_PROGRESS_LABEL = 'in-progress';

function normalizeLabels(raw: ProviderIssue['labels']): string[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((entry) => (typeof entry === 'string' ? entry : entry.name ?? entry.name_and_description ?? ''))
    .filter((name) => name.length > 0);
}

function normalizeIssue(raw: ProviderIssue): ChatIssueSummary & { body: string | null; state: string } {
  return {
    number: raw.number ?? raw.iid ?? 0,
    title: raw.title,
    body: raw.body ?? raw.description ?? null,
    state: raw.state,
    url: raw.url ?? raw.web_url ?? null,
    labels: normalizeLabels(raw.labels),
    updatedAt: raw.updatedAt ?? raw.updated_at ?? null
  };
}

/** Open issues for a profile's repo, newest-first. Cached per profile. */
export async function listChatIssues(
  profileInfo: ForgeProject,
  limit = 30
): Promise<ChatIssueSummary[]> {
  const cacheKey = `${profileInfo.provider}:${profileInfo.repo}:${profileInfo.local_path}:${limit}`;
  return issuesCache.get(cacheKey, async () => {
    const isGitLab = profileInfo.provider === 'gitlab';
    const { stdout } = isGitLab
      ? await execProjectCli('glab', ['issue', 'list', '--output=json'], profileInfo)
      : await execProjectCli('gh', ['issue', 'list', '--json', 'number,title,url,labels,updatedAt,state', `--limit=${limit}`], profileInfo);
    const parsed = JSON.parse(stdout) as ProviderIssue[];
    const isOpen = (state: string): boolean => {
      const normalized = state.toLowerCase();
      // GitHub reports OPEN/CLOSED; GitLab opened/closed. Absent state (some
      // list shapes) defaults to open: `issue list` without --state=all only
      // returns open issues anyway.
      return normalized === 'open' || normalized === 'opened' || state === '';
    };
    return parsed
      .map((raw) => normalizeIssue(raw))
      .filter((issue) => isOpen(issue.state))
      .sort((a, b) => b.number - a.number)
      .map(({ number, title, url, labels, updatedAt }) => ({ number, title, url, labels, updatedAt }));
  });
}

export async function fetchChatIssue(
  profileInfo: ForgeProject,
  issueNumber: number
): Promise<ReturnType<typeof normalizeIssue>> {
  const isGitLab = profileInfo.provider === 'gitlab';
  const { stdout } = isGitLab
    ? await execProjectCli('glab', ['issue', 'view', String(issueNumber), '--output=json'], profileInfo)
    : await execProjectCli('gh', ['issue', 'view', String(issueNumber), '--json', 'number,title,body,state,url,labels'], profileInfo);
  return normalizeIssue(JSON.parse(stdout) as ProviderIssue);
}

/** Provider state lookup shared by issue-chat creation and maintenance. */
export async function fetchChatIssueState(
  profileInfo: ForgeProject,
  issueNumber: number
): Promise<string> {
  return (await fetchChatIssue(profileInfo, issueNumber)).state;
}

export async function fetchLinkedChatIssues(
  profileInfo: ForgeProject,
  issueNumbers: number[]
): Promise<ChatIssueSummary[]> {
  const issues = await Promise.all(issueNumbers.map(number => fetchChatIssue(profileInfo, number).catch(() => null)));
  return issues.filter((issue): issue is NonNullable<typeof issue> => issue !== null);
}

async function labelExists(
  profileInfo: ForgeProject,
  label: string
): Promise<boolean> {
  try {
    const isGitLab = profileInfo.provider === 'gitlab';
    const { stdout } = isGitLab
      ? await execProjectCli('glab', ['label', 'list', '--output=json'], profileInfo)
      : // gh defaults to 30 labels per page -- repos routinely have more
        // (this one has ~35), which made the in-progress check miss.
      await execProjectCli('gh', ['label', 'list', '--json', 'name', '--limit', '200'], profileInfo);
    const parsed = JSON.parse(stdout) as { name?: string }[];
    return parsed.some((entry) => entry.name === label);
  } catch {
    return false;
  }
}

/** Marks the issue in progress: assignee @me always, the in-progress label
 * when it exists. Failures surface -- silently claiming an issue nobody
 * can see claimed is worse than an error. */
async function markIssueInProgress(
  profileInfo: ForgeProject,
  issueNumber: number,
  label: string
): Promise<void> {
  const isGitLab = profileInfo.provider === 'gitlab';
  const args = isGitLab
    ? ['issue', 'update', String(issueNumber), '--assignee', '@me']
    : ['issue', 'edit', String(issueNumber), '--add-assignee', '@me'];
  if (await labelExists(profileInfo, label)) {
    if (isGitLab) args.push('--label', label);
    else args.push('--add-label', label);
  }
  await execProjectCli(isGitLab ? 'glab' : 'gh', args, profileInfo);
}

export function issueBranchName(repoId: string, issueNumber: number): string {
  return `gah/issue/${repoId}-${issueNumber}`;
}

export interface StartIssueChatInput {
  profile: string;
  profileInfo: ProfileSummary;
  issueNumber: number;
  backend: string;
  model?: string | null;
  /** Label used to mark the issue in progress (default: in-progress). */
  inProgressLabel?: string;
  /** Store override (tests). */
  storeOptions?: ChatSessionStoreOptions;
  /** Creates the session on the selected node. Absent means a local session. */
  create?: (settings: { title: string; branch: string }) => Promise<ChatSessionSummary>;
}

export interface StartIssueChatResult {
  session: ChatSessionSummary;
  /** True when a live session for this issue already existed and was
   * returned as-is (idempotent grab). */
  existing: boolean;
}

export async function startIssueChat(input: StartIssueChatInput): Promise<StartIssueChatResult> {
  const { profile, profileInfo, issueNumber, backend } = input;
  const storeOptions = input.storeOptions ?? chatSessionStoreOptions;
  const canonicalBranch = issueBranchName(profileInfo.repo_id, issueNumber);

  // Idempotent grab: one live session per (profile, issue).
  const live = listSessions(profile, storeOptions).find(
    (session) => session.archivedAt === null && session.branch === canonicalBranch
  );
  if (live) return { session: live, existing: true };

  const issue = await fetchChatIssue(profileInfo, issueNumber);
  const normalizedState = issue.state.toLowerCase();
  if (normalizedState !== 'open' && normalizedState !== 'opened') {
    throw new Error(`Issue #${issueNumber} is ${issue.state}, not open`);
  }
  await markIssueInProgress(profileInfo, issueNumber, input.inProgressLabel ?? ISSUE_IN_PROGRESS_LABEL);

  // After an archive the canonical branch still exists (branches survive
  // by design); a fresh grab for the same issue gets a suffixed branch.
  // Central can't list a worker's branches, so a remote grab checks central's
  // own session records for an earlier use of the branch instead.
  const branchTaken = !profileInfo.local_path
    ? listSessions(profile, storeOptions).some((session) => session.branch === canonicalBranch)
    : !!(await execProviderCli('git', ['branch', '--list', canonicalBranch], profileInfo.local_path)).stdout.trim();
  const title = `#${issueNumber} ${issue.title}`;
  const branch = branchTaken ? `${canonicalBranch}-${Date.now().toString(36)}` : canonicalBranch;
  const session = input.create
    ? await input.create({ title, branch })
    : await createSession(
      { profile, profileInfo, backend, issueNumber, model: input.model ?? null, title, branch },
      storeOptions
    );

  // The issue is the conversation's opening message.
  const issueText = [
    `#${issue.number} ${issue.title}`,
    issue.body?.trim() ?? '',
    issue.url ? `\n(${issue.url})` : ''
  ].filter((part) => part.length > 0).join('\n\n');
  seedOpeningMessage(profile, issueText, { stateDir: storeOptions.stateDir, sessionId: session.id });

  return { session, existing: false };
}
