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
import { profileStorage, type SettleDetails } from './chatSessions.js';
import { fetchChatIssueState } from './issueChats.js';

const DAY_MS = 86_400_000;

export interface ChatMaintenanceDeps {
  listProfiles(): Promise<ProfileSummary[]>;
  sync(profile: string): Promise<MergeRequest[]>;
  issueState(profile: ProfileSummary, issueNumber: number): Promise<string>;
  listSessions(profile: string): ChatSessionSummary[];
  archive(profile: string, sessionId: string, settlement?: { reason: 'merged' | 'closed' | 'delivered' }, details?: SettleDetails): Promise<ChatSessionSummary>;
  isActive(profile: string, sessionId: string): boolean;
  now(): number;
}

const defaultDeps: ChatMaintenanceDeps = {
  listProfiles: runProfileList,
  sync: (profile) => runSync({ profile }),
  issueState: fetchChatIssueState,
  listSessions: listChatSessions,
  archive: async (profile, sessionId, settlement, details) => archiveChatSession(profile, sessionId, settlement, details),
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
      // Branch → terminal MR, so the settled event can record WHICH PR
      // proved the work done (#1036).
      const mrByBranch = new Map(mergeRequests.map((mr) => [mr.branch, mr] as const));
      for (const candidate of profileCandidates) {
        const settledSession = sessions.find((session) => session.id === candidate.sessionId);
        let details: SettleDetails | undefined;
        if (candidate.outcome === 'settled' && settledSession) {
          const mr = mrByBranch.get(settledSession.branch);
          if (mr && (candidate.reason === 'merged' || candidate.reason === 'closed')) {
            details = {
              pullRequest: {
                id: mr.id,
                url: mr.url,
                sourceSha: candidate.reason === 'merged'
                  ? mr.merge_commit_sha ?? null
                  : mr.source_sha ?? null
              }
            };
          } else if (candidate.reason === 'closed') {
            const issueNumber = issueNumberForBranch(settledSession.branch, profile.repo_id);
            if (issueNumber !== null) details = { issue: { number: issueNumber } };
          }
        }
        archived.push(await deps.archive(
          profile.name,
          candidate.sessionId,
          candidate.outcome === 'settled'
            ? { reason: candidate.reason === 'idle' ? 'delivered' : candidate.reason }
            : undefined,
          details
        ));
      }
    }
  }

  return { dryRun: options.dryRun, profiles: storage, candidates, sessions: archived, warnings };
}

// ---------------------------------------------------------------------------
// Bounded-interval settle sweep (#1036)
//
// The sweep above ran only through the daily gah-prune timer and the
// dashboard's Reclaim button, so a chat whose PR merged at noon sat `live`
// until the next day. gah-server now runs the same sweep on a bounded
// interval: only profiles with live sessions are synced, overlapping runs
// are dropped, and failures are logged not thrown (fail-open, like the rest
// of maintenance).

const DEFAULT_MAINTENANCE_INTERVAL_MS = 15 * 60_000;
const FIRST_TICK_DELAY_MS = 30_000;

function maintenanceIntervalMs(): number {
  const raw = Number.parseInt(process.env.GAH_CHAT_MAINTENANCE_INTERVAL_MS ?? '', 10);
  return Number.isFinite(raw) && raw > 0 ? raw : DEFAULT_MAINTENANCE_INTERVAL_MS;
}

/** One scheduler pass: sweep only profiles that still have live sessions.
 * Returns the profiles that were swept (for tests/logs). */
export async function runChatMaintenanceTick(
  deps: ChatMaintenanceDeps = defaultDeps
): Promise<string[]> {
  const profiles = await deps.listProfiles();
  const swept: string[] = [];
  for (const profile of profiles) {
    const hasLive = deps.listSessions(profile.name).some((session) => session.outcome === 'live');
    if (!hasLive) continue;
    swept.push(profile.name);
    const result = await reclaimChatSessions({ profile: profile.name, dryRun: false }, deps);
    for (const session of result.sessions) {
      const reason = session.settledReason ?? 'archived';
      console.log(`[chat-maintenance] ${session.outcome} session ${session.id} (${reason}) for profile ${profile.name}`);
    }
  }
  return swept;
}

let maintenanceTimer: ReturnType<typeof setInterval> | null = null;
let maintenanceRunning = false;

export function startChatMaintenanceScheduler(deps: ChatMaintenanceDeps = defaultDeps): void {
  if (maintenanceTimer) return;
  const tick = () => {
    if (maintenanceRunning) return;
    maintenanceRunning = true;
    runChatMaintenanceTick(deps)
      .catch((error) => console.warn(`[chat-maintenance] sweep failed: ${error instanceof Error ? error.message : String(error)}`))
      .finally(() => { maintenanceRunning = false; });
  };
  maintenanceTimer = setInterval(tick, maintenanceIntervalMs());
  maintenanceTimer.unref?.();
  setTimeout(tick, FIRST_TICK_DELAY_MS).unref?.();
}

export function stopChatMaintenanceScheduler(): void {
  if (maintenanceTimer) {
    clearInterval(maintenanceTimer);
    maintenanceTimer = null;
  }
}
