/**
 * Actual-usage rollup from the manager-chat session logs (#940).
 *
 * The dispatch ledger only records gah-dispatch attempts, and the
 * account-level quota checks depend on each backend CLI reporting plan
 * state -- so neither answers "what did my backends actually burn through
 * GAH?" once nearly all work happens in manager chat. But every assistant
 * message in a session log already carries the usage the backend reported
 * for that turn. This module aggregates exactly that: a scan of a profile's
 * session logs grouped by backend + model + UTC day. No Rust changes, no
 * new capture path -- the data exists; this makes it visible.
 */

import { existsSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import type { UsageRollupRow, UsageRollupSummary, UsageRollupTicketRow } from '@git-agent-harness/contracts';
import { readLog } from './sessionLog.js';
import { chatSessionStoreOptions, listSessions, stateBase } from './chatSessions.js';

const DAY_MS = 86_400_000;

function utcDay(timestamp: number): string {
  return new Date(timestamp).toISOString().slice(0, 10);
}

/** A ticket label for a session: the issue number its branch carries
 * (gah/issue/<repo>-<n>), else the branch itself (PR chats bind to the
 * PR's head branch; plain chats carry their own chat branch). */
export function ticketLabelForBranch(branch: string | null): string | null {
  if (!branch) return null;
  const match = /^gah\/issue\/[^-]+-(\d+)/.exec(branch);
  return match ? `#${match[1]}` : branch;
}

/** Session log ids present for a profile: the session-* dirs under the
 * profile's chat state dir. Cheap directory listing, no CLI spawns. */
function sessionLogIds(profile: string, stateDir?: string): string[] {
  const profileDir = join(stateDir ?? stateBase(), `project-${encodeURIComponent(profile)}`);
  if (!existsSync(profileDir)) return [];
  return readdirSync(profileDir)
    .filter((entry) => entry.startsWith('session-'))
    .map((entry) => decodeURIComponent(entry.slice('session-'.length)));
}

export function usageRollup(profile: string, days: number, opts?: { stateDir?: string; now?: () => number }): UsageRollupSummary {
  const now = (opts?.now ?? Date.now)();
  const since = now - days * DAY_MS;
  const stateDir = opts?.stateDir ?? chatSessionStoreOptions.stateDir;

  const totals = new Map<string, UsageRollupRow>();
  const ticketTotals = new Map<string, UsageRollupTicketRow>();
  let unattributedTurns = 0;

  // Per-ticket rollup (#940 "cost per ticket"): sessions carry their
  // branch, and issue/PR chats branch for their ticket -- so joining the
  // usage rows back to the session index gives ticket-level burn. PR chats
  // bind to the PR's head branch (not a gah/pr branch), so those roll up
  // under the branch name itself.
  const branchBySession = new Map<string, { branch: string | null; title: string | null }>();
  try {
    for (const session of listSessions(profile, { stateDir })) {
      branchBySession.set(session.id, { branch: session.branch, title: session.title });
    }
  } catch {
    // No session index: ticket rollup stays empty, the day rows still work.
  }

  for (const sessionId of sessionLogIds(profile, stateDir)) {
    const meta = branchBySession.get(sessionId);
    const label = ticketLabelForBranch(meta?.branch ?? null) ?? 'default conversation';
    let ticketRow = ticketTotals.get(label);
    if (!ticketRow) {
      ticketRow = { ticket: label, title: meta?.title ?? null, turns: 0, total_tokens: 0, estimated_cost_usd: 0, backends: {} };
      ticketTotals.set(label, ticketRow);
    }
    if (!ticketRow.title && meta?.title) ticketRow.title = meta.title;

    let events;
    try {
      events = readLog(profile, { stateDir, sessionId });
    } catch {
      continue;
    }
    for (const event of events) {
      if (event.type !== 'assistant/message') continue;
      if (event.timestamp < since) continue;
      const usage = event.usage;
      if (!usage || (usage.total_tokens === null && usage.input_tokens === null && usage.output_tokens === null)) {
        unattributedTurns += 1;
        continue;
      }
      const tokens = usage.total_tokens ?? (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0);

      const key = `${event.backend}\u0000${event.model ?? ''}\u0000${utcDay(event.timestamp)}`;
      let row = totals.get(key);
      if (!row) {
        row = {
          backend: event.backend,
          model: event.model,
          day: utcDay(event.timestamp),
          turns: 0,
          input_tokens: 0,
          output_tokens: 0,
          total_tokens: 0,
          estimated_cost_usd: 0
        };
        totals.set(key, row);
      }
      row.turns += 1;
      row.input_tokens += usage.input_tokens ?? 0;
      row.output_tokens += usage.output_tokens ?? 0;
      row.total_tokens += tokens;
      row.estimated_cost_usd += usage.estimated_cost_usd ?? 0;

      ticketRow.turns += 1;
      ticketRow.total_tokens += tokens;
      ticketRow.estimated_cost_usd += usage.estimated_cost_usd ?? 0;
      ticketRow.backends[event.backend] = (ticketRow.backends[event.backend] ?? 0) + tokens;
    }
  }

  const rows = [...totals.values()].sort((a, b) =>
    a.day === b.day
      ? (a.backend === b.backend ? (a.model ?? '').localeCompare(b.model ?? '') : a.backend.localeCompare(b.backend))
      : (a.day < b.day ? 1 : -1)
  );

  return {
    profile,
    since,
    generated_at: now,
    rows,
    unattributed_turns: unattributedTurns,
    tickets: [...ticketTotals.values()]
      .filter((row) => row.turns > 0)
      .sort((a, b) => b.total_tokens - a.total_tokens)
  };
}
