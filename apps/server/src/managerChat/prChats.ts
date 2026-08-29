/**
 * PR → chat sessions: grab a provider pull request and open a GAH chat
 * seeded with it. Browsing is read-only — unlike issue chats nothing is
 * created at the provider, no branch is cut, and no worktree is
 * materialized; turns run in the profile checkout. The session rides the
 * existing chat-session machinery with the PR's head branch recorded as
 * the session branch, so the maintenance settle-by-branch sweep sees it
 * when the PR merges or closes.
 *
 * Starting is idempotent per (profile, PR): a live session tagged with the
 * PR number is returned as-is instead of creating a second one.
 */

import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { ChatSessionSummary, ProfileSummary } from '@git-agent-harness/contracts';
import { appendEvents } from './sessionLog.js';
import {
  chatSessionStoreOptions,
  createSession,
  listSessions,
  type ChatSessionStoreOptions
} from './chatSessions.js';

const execFileAsync = promisify(execFile);

export interface ChatPrSummary {
  number: number;
  title: string;
  url: string | null;
  author: string | null;
  headRefName: string | null;
  isDraft: boolean;
  reviewState: string | null;
  updatedAt: string | null;
}

interface ProviderPr {
  number?: number;
  iid?: number;
  title: string;
  body?: string | null;
  state: string;
  url?: string | null;
  web_url?: string | null;
  author?: { login?: string; username?: string } | null;
  headRefName?: string | null;
  source_branch?: string | null;
  isDraft?: boolean;
  draft?: boolean;
  updatedAt?: string | null;
  updated_at?: string | null;
  reviewDecision?: string | null;
}

function execCli(command: string, args: string[], cwd: string): Promise<{ stdout: string }> {
  return execFileAsync(command, args, { cwd, maxBuffer: 16 * 1024 * 1024 });
}

function normalizePr(raw: ProviderPr): ChatPrSummary & { body: string | null; state: string } {
  return {
    number: raw.number ?? raw.iid ?? 0,
    title: raw.title,
    body: raw.body ?? null,
    state: raw.state,
    url: raw.url ?? raw.web_url ?? null,
    author: raw.author?.login ?? raw.author?.username ?? null,
    headRefName: raw.headRefName ?? raw.source_branch ?? null,
    isDraft: raw.isDraft ?? raw.draft ?? false,
    reviewState: raw.reviewDecision ?? null,
    updatedAt: raw.updatedAt ?? raw.updated_at ?? null
  };
}

/** Open PRs for a profile's repo, newest-first. */
export async function listChatPrs(
  profileInfo: Pick<ProfileSummary, 'provider' | 'repo' | 'local_path'>,
  limit = 30
): Promise<ChatPrSummary[]> {
  const isGitLab = profileInfo.provider === 'gitlab';
  const { stdout } = isGitLab
    ? await execCli('glab', ['mr', 'list', '--output=json'], profileInfo.local_path)
    : await execCli('gh', ['pr', 'list', '--json', 'number,title,url,headRefName,isDraft,updatedAt,state,author,reviewDecision', `--limit=${limit}`], profileInfo.local_path);
  const parsed = JSON.parse(stdout) as ProviderPr[];
  const isOpen = (state: string): boolean => {
    const normalized = state.toLowerCase();
    // GitHub reports OPEN/MERGED/CLOSED; GitLab opened/closed/merged.
    return normalized === 'open' || normalized === 'opened';
  };
  return parsed
    .map((raw) => normalizePr(raw))
    .filter((pr) => isOpen(pr.state))
    .sort((a, b) => b.number - a.number);
}

async function fetchPr(
  profileInfo: Pick<ProfileSummary, 'provider' | 'local_path'>,
  prNumber: number
): Promise<ReturnType<typeof normalizePr>> {
  const isGitLab = profileInfo.provider === 'gitlab';
  const { stdout } = isGitLab
    ? await execCli('glab', ['mr', 'view', String(prNumber), '--output=json'], profileInfo.local_path)
    : await execCli('gh', ['pr', 'view', String(prNumber), '--json', 'number,title,body,state,url,headRefName,isDraft,author,reviewDecision'], profileInfo.local_path);
  return normalizePr(JSON.parse(stdout) as ProviderPr);
}

export interface StartPrChatInput {
  profile: string;
  profileInfo: ProfileSummary;
  prNumber: number;
  backend: string;
  model?: string | null;
  /** Store override (tests). */
  storeOptions?: ChatSessionStoreOptions;
}

export interface StartPrChatResult {
  session: ChatSessionSummary;
  /** True when a live session for this PR already existed and was
   * returned as-is (idempotent open). */
  existing: boolean;
}

export async function startPrChat(input: StartPrChatInput): Promise<StartPrChatResult> {
  const { profile, profileInfo, prNumber, backend } = input;
  const storeOptions = input.storeOptions ?? chatSessionStoreOptions;
  const pr = await fetchPr(profileInfo, prNumber);
  // Record the PR's head branch for maintenance settlement; PR identity is
  // persisted separately because issue/general chats may use the same branch.
  const canonicalBranch = pr.headRefName ?? `gah/pr/${profileInfo.repo_id}-${prNumber}`;

  // Idempotent open: one explicitly tagged live session per (profile, PR).
  // Issue and general chats may share the head branch but have no PR identity.
  const live = listSessions(profile, storeOptions).find(
    (session) => session.archivedAt === null
      && session.prNumber === prNumber
  );
  if (live) return { session: live, existing: true };

  const normalizedState = pr.state.toLowerCase();
  if (normalizedState !== 'open' && normalizedState !== 'opened') {
    throw new Error(`PR #${prNumber} is ${pr.state}, not open`);
  }

  // Worktree-less on purpose: the conversation is about the PR's diff, not
  // a fresh branch from it, and nothing at the provider may be touched.
  const session = await createSession(
    {
      profile,
      profileInfo,
      prNumber,
      backend,
      model: input.model ?? null,
      title: `#${prNumber} ${pr.title}`,
      branch: canonicalBranch,
      worktree: false
    },
    storeOptions
  );

  // Seed the log: the PR is the opening message of the conversation --
  // rendered in the transcript and replayed into every backend's context.
  const now = Date.now();
  const prText = [
    `#${pr.number} ${pr.title}`,
    pr.body?.trim() ?? '',
    pr.headRefName ? `Head branch: ${pr.headRefName}` : '',
    pr.url ? `\n(${pr.url})` : ''
  ].filter((part) => part.length > 0).join('\n\n');
  appendEvents(profile, [
    { type: 'turn/start', seq: 1, turn: 1, timestamp: now },
    { type: 'user/message', seq: 2, turn: 1, text: prText, source: 'prompt', timestamp: now },
    { type: 'turn/end', seq: 3, turn: 1, reason: { kind: 'complete' }, timestamp: now }
  ], { stateDir: storeOptions.stateDir, sessionId: session.id });

  return { session, existing: false };
}
