/**
 * Seed watchdog (#1042): an issue->chat (or PR->chat) session is seeded
 * with the issue body and then waits for the orchestrator's implement
 * instruction as a second turn. When that dispatch never arrives (the
 * orchestrator died or paused between seeding and dispatch), the session
 * sat silently `live` forever: no event, no error, nothing in the server
 * log. The watchdog scans on a bounded interval and marks such sessions
 * with a durable harness/error event so the stall is visible in the
 * dashboard instead of guesswork.
 */

import { readdirSync } from 'node:fs';
import { appendEvents, readLog } from './sessionLog.js';
import { chatSessionStoreOptions, listSessions, stateBase } from './chatSessions.js';

export const SEED_WATCHDOG_MARKER = 'Seeded session was never dispatched';

const DEFAULT_DEADLINE_MS = 2 * 60_000;
const DEFAULT_INTERVAL_MS = 30_000;

function deadlineMs(): number {
  const raw = Number.parseInt(process.env.GAH_SEED_WATCHDOG_DEADLINE_MS ?? '', 10);
  return Number.isFinite(raw) && raw > 0 ? raw : DEFAULT_DEADLINE_MS;
}

function intervalMs(): number {
  const raw = Number.parseInt(process.env.GAH_SEED_WATCHDOG_INTERVAL_MS ?? '', 10);
  return Number.isFinite(raw) && raw > 0 ? raw : DEFAULT_INTERVAL_MS;
}

/** The seed shape issue/PR chats write: exactly one closed turn holding the
 * seeded prompt and nothing else. Any assistant activity or a second turn
 * means the dispatch arrived. */
function isUndispatchedSeed(events: ReturnType<typeof readLog>): boolean {
  const turns = events.filter((event) => event.type === 'turn/start').length;
  const activity = events.some((event) =>
    event.type === 'assistant/chunk'
    || event.type === 'assistant/message'
    || event.type === 'tool/call'
    || event.type === 'tool/result'
  );
  return turns === 1 && !activity;
}

export interface SeedWatchdogOptions {
  now?: () => number;
  /** Overrides the default deadline (tests, or env GAH_SEED_WATCHDOG_DEADLINE_MS). */
  deadlineMs?: number;
}

/** One scan across every project's live sessions. Returns the flagged
 * `profile/sessionId` pairs; already-flagged and dispatched sessions are
 * skipped, so rescans are free. */
export function runSeedWatchdogScan(options: SeedWatchdogOptions = {}): string[] {
  const now = (options.now ?? Date.now)();
  const deadline = options.deadlineMs ?? deadlineMs();
  const flagged: string[] = [];

  let profileDirs: string[] = [];
  try {
    profileDirs = readdirSync(stateBase()).filter((entry) => entry.startsWith('project-'));
  } catch {
    return flagged;
  }

  for (const dir of profileDirs) {
    const profile = decodeURIComponent(dir.slice('project-'.length));
    let sessions;
    try {
      sessions = listSessions(profile);
    } catch {
      continue;
    }
    for (const session of sessions) {
      if (session.outcome !== 'live') continue;
      if (now - session.createdAt < deadline) continue;
      const logOpts = { stateDir: chatSessionStoreOptions.stateDir, sessionId: session.id };
      let events;
      try {
        events = readLog(profile, logOpts);
      } catch {
        continue;
      }
      if (!isUndispatchedSeed(events)) continue;
      if (events.some((event) => event.type === 'harness/error' && event.text.startsWith(SEED_WATCHDOG_MARKER))) continue;
      const nextSeq = events.reduce((highest, event) => Math.max(highest, event.seq), 0) + 1;
      appendEvents(profile, [{
        type: 'harness/error',
        seq: nextSeq,
        turn: 0,
        text: `${SEED_WATCHDOG_MARKER} within ${Math.round(deadline / 1000)}s of creation (no implement instruction arrived; the orchestrator may have stopped). Re-send the task to this session or archive it.`,
        timestamp: now
      }], logOpts);
      flagged.push(`${profile}/${session.id}`);
    }
  }
  return flagged;
}

let watchdogTimer: ReturnType<typeof setInterval> | null = null;
let watchdogRunning = false;

export function startSeedWatchdogScheduler(): void {
  if (watchdogTimer) return;
  const tick = () => {
    if (watchdogRunning) return;
    watchdogRunning = true;
    try {
      const flagged = runSeedWatchdogScan();
      for (const id of flagged) {
        console.warn(`[seed-watchdog] flagged ${id}: seeded session never received its dispatch turn`);
      }
    } catch (error) {
      console.warn(`[seed-watchdog] scan failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      watchdogRunning = false;
    }
  };
  watchdogTimer = setInterval(tick, intervalMs());
  watchdogTimer.unref?.();
}

export function stopSeedWatchdogScheduler(): void {
  if (watchdogTimer) {
    clearInterval(watchdogTimer);
    watchdogTimer = null;
  }
}
