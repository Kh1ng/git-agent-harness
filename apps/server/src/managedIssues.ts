// Managed status for the dashboard's start paths (the Kanban Assign button
// over the socket, and POST /api/dispatch). An issue the status snapshot
// reports as `managed` (the managed label, or an assignee other than the
// loop's own login) belongs to a manager, so an implementation job may not be
// started on it from here. The loop's own intake applies the same rule in
// src/dispatch/issues.rs; this is the same fence on the other door.
import type { StatusSnapshot } from '@git-agent-harness/contracts';
import { GAHError } from '@git-agent-harness/shared';
import { runStatus } from './gahCli.js';

export const MANAGED_REASON_CODE = 'managed';
export const ISSUE_MANAGED_ERROR_CODE = 'ISSUE_MANAGED';

/** Job kinds that would put a second worker on the issue's code. */
const IMPLEMENTATION_MODES = new Set(['fix', 'improve']);
/** How long a start waits for the status snapshot before going ahead without the check. */
export const STATUS_TIMEOUT_MS = 5_000;

/** `#123`, `123` or `TICKET-123` as `#123`; anything else is not an issue. */
export function issueWorkId(target: string | undefined): string | null {
  const trimmed = target?.trim().toLowerCase() ?? '';
  const digits = trimmed.startsWith('#') ? trimmed.slice(1) : trimmed.startsWith('ticket-') ? trimmed.slice('ticket-'.length) : trimmed;
  if (digits.length === 0 || digits.length > 12 || !/^\d+$/.test(digits)) return null;
  return `#${Number.parseInt(digits, 10)}`;
}

/** Why `workId` may not be started here, from the status snapshot; null when it may. */
export function managedHold(status: Pick<StatusSnapshot, 'issue_intake_rejections'>, workId: string): string | null {
  const rejection = (status.issue_intake_rejections ?? []).find(
    (candidate) => candidate.reason_code === MANAGED_REASON_CODE && candidate.work_id !== null && issueWorkId(candidate.work_id) === workId
  );
  if (!rejection) return null;
  const reason = rejection.reason.replace(/^managed:\s*/i, '');
  return `${workId} is managed (${reason}); a manager owns it, so it cannot be started from here`;
}

export type DispatchRequest = { profile: string; mode?: string; target?: string };

/**
 * Throws `GAHError(ISSUE_MANAGED)` when an implementation dispatch names a
 * managed issue. Anything else passes: other modes, non-issue targets, and a
 * status snapshot that cannot be read or takes longer than `timeoutMs` (the
 * fence is only as good as the snapshot, and an unreachable forge or a cold
 * cache must not freeze every manual start). The snapshot is the cached one
 * the dashboard shows, so a warm server answers at once.
 */
export async function assertIssueNotManaged(
  request: DispatchRequest,
  loadStatus: (profile: string) => Promise<Pick<StatusSnapshot, 'issue_intake_rejections'>> = (profile) => runStatus(profile),
  timeoutMs = STATUS_TIMEOUT_MS
): Promise<void> {
  if (!request.mode || !IMPLEMENTATION_MODES.has(request.mode)) return;
  const workId = issueWorkId(request.target);
  if (!workId) return;
  let status: Pick<StatusSnapshot, 'issue_intake_rejections'>;
  let timer: NodeJS.Timeout | undefined;
  try {
    status = await Promise.race([
      loadStatus(request.profile),
      new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`status took longer than ${timeoutMs} ms`)), timeoutMs); })
    ]);
  } catch (error) {
    console.warn(`[managed] could not read status for ${request.profile}; starting ${workId} without the managed check: ${error instanceof Error ? error.message : String(error)}`);
    return;
  } finally {
    if (timer) clearTimeout(timer);
  }
  const hold = managedHold(status, workId);
  if (hold) throw new GAHError(hold, ISSUE_MANAGED_ERROR_CODE, { workId });
}
