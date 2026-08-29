import type {
  ChatReclaimCandidate,
  ChatReclaimResult,
  ChatSessionSummary,
  MergeRequest,
  ProfileSummary
} from '@git-agent-harness/contracts';
import { runProfileList, runSync } from '../gahCli.js';
import {
  archiveChatSession,
  isChatSessionActive,
  listChatSessions
} from './ManagerChatManager.js';
import { profileStorage } from './chatSessions.js';
import { fetchChatIssueState } from './issueChats.js';

const DAY_MS = 86_400_000;

export interface ChatMaintenanceDeps {
  listProfiles(): Promise<ProfileSummary[]>;
  sync(profile: string): Promise<MergeRequest[]>;
  issueState(profile: ProfileSummary, issueNumber: number): Promise<string>;
  listSessions(profile: string): ChatSessionSummary[];
  archive(profile: string, sessionId: string, settlement?: { reason: 'merged' | 'closed' | 'delivered' }): Promise<ChatSessionSummary>;
  isActive(profile: string, sessionId: string): boolean;
  now(): number;
}

const defaultDeps: ChatMaintenanceDeps = {
  listProfiles: runProfileList,
  sync: (profile) => runSync({ profile }),
  issueState: fetchChatIssueState,
  listSessions: listChatSessions,
  archive: async (profile, sessionId, settlement) => archiveChatSession(profile, sessionId, settlement),
  isActive: isChatSessionActive,
  now: Date.now
};

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function issueNumberForBranch(branch: string, repoId: string): number | null {
  const match = new RegExp(`^gah/issue/${escapeRegExp(repoId)}-(\\d+)(?:-|$)`).exec(branch);
  return match ? Number.parseInt(match[1], 10) : null;
}

export function selectReclaimCandidates(input: {
  profile: ProfileSummary;
  sessions: ChatSessionSummary[];
  mergeRequests: MergeRequest[];
  closedIssues: ReadonlySet<number>;
  activeSessionIds: ReadonlySet<string>;
  now: number;
}): ChatReclaimCandidate[] {
  const terminalByBranch = new Map(
    input.mergeRequests
      .filter((mr) => mr.classification === 'MERGED' || mr.classification === 'CLOSED_UNMERGED')
      .map((mr) => [mr.branch, mr.classification] as const)
  );
  const idleCutoff = input.now - (input.profile.chat_session_idle_days ?? 14) * DAY_MS;
  const candidates: ChatReclaimCandidate[] = [];
  for (const session of input.sessions) {
    if (session.outcome !== 'live' || input.activeSessionIds.has(session.id)) continue;
    const terminal = terminalByBranch.get(session.branch);
    if (terminal) {
      candidates.push({
        profile: input.profile.name,
        sessionId: session.id,
        outcome: 'settled',
        reason: terminal === 'MERGED' ? 'merged' : 'closed',
        reclaimBytes: 0
      });
      continue;
    }
    const issueNumber = issueNumberForBranch(session.branch, input.profile.repo_id);
    if (issueNumber !== null && input.closedIssues.has(issueNumber)) {
      candidates.push({
        profile: input.profile.name,
        sessionId: session.id,
        outcome: 'settled',
        reason: 'closed',
        reclaimBytes: 0
      });
      continue;
    }
    if (session.lastActiveAt <= idleCutoff) {
      candidates.push({
        profile: input.profile.name,
        sessionId: session.id,
        outcome: 'archived',
        reason: 'idle',
        reclaimBytes: 0
      });
    }
  }
  return candidates;
}

/** One coherent sweep used by dry-run visibility, the Reclaim now button,
 * and the existing daily gah-prune service entrypoint. */
export async function reclaimChatSessions(
  options: { profile?: string; dryRun: boolean },
  deps: ChatMaintenanceDeps = defaultDeps
): Promise<ChatReclaimResult> {
  const allProfiles = await deps.listProfiles();
  const profiles = options.profile
    ? allProfiles.filter((profile) => profile.name === options.profile)
    : allProfiles;
  if (options.profile && profiles.length === 0) throw new Error(`Profile '${options.profile}' not found`);

  const warnings: string[] = [];
  const candidates: ChatReclaimCandidate[] = [];
  const storage = [];
  const archived: ChatSessionSummary[] = [];

  for (const profile of profiles) {
    const sessions = deps.listSessions(profile.name);
    let mergeRequests: MergeRequest[] = [];
    try {
      mergeRequests = await deps.sync(profile.name);
    } catch (error) {
      warnings.push(`[${profile.name}] provider sync failed: ${error instanceof Error ? error.message : String(error)}`);
    }

    const closedIssues = new Set<number>();
    const issueNumbers = [...new Set(sessions
      .filter((session) => session.outcome === 'live')
      .map((session) => issueNumberForBranch(session.branch, profile.repo_id))
      .filter((number): number is number => number !== null))];
    for (const issueNumber of issueNumbers) {
      try {
        const state = (await deps.issueState(profile, issueNumber)).toLowerCase();
        if (state === 'closed') closedIssues.add(issueNumber);
      } catch (error) {
        warnings.push(`[${profile.name}] issue #${issueNumber} lookup failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    }

    const profileCandidates = selectReclaimCandidates({
      profile,
      sessions,
      mergeRequests,
      closedIssues,
      activeSessionIds: new Set(sessions.filter((session) => deps.isActive(profile.name, session.id)).map((session) => session.id)),
      now: deps.now()
    });
    const reclaimIds = new Set(profileCandidates.map((candidate) => candidate.sessionId));
    const profileStorageSummary = await profileStorage(profile.name, profile.chat_session_idle_days ?? 14, reclaimIds, undefined, deps.now());
    const bytesBySession = new Map(profileStorageSummary.sessions.map((session) => [session.sessionId, session.projectedReclaimBytes]));
    for (const candidate of profileCandidates) {
      candidate.reclaimBytes = bytesBySession.get(candidate.sessionId) ?? 0;
    }
    candidates.push(...profileCandidates);
    storage.push(profileStorageSummary);

    if (!options.dryRun) {
      for (const candidate of profileCandidates) {
        archived.push(await deps.archive(
          profile.name,
          candidate.sessionId,
          candidate.outcome === 'settled'
            ? { reason: candidate.reason === 'idle' ? 'delivered' : candidate.reason }
            : undefined
        ));
      }
    }
  }

  return { dryRun: options.dryRun, profiles: storage, candidates, sessions: archived, warnings };
}
