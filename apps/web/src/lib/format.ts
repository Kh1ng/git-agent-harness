/**
 * Formatting helpers that enforce the one non-negotiable rule across every
 * page: a missing/unknown value renders as "Unknown" text, never as 0, 0%,
 * or an empty progress bar that reads as "nothing." See Phase 5's
 * "Unknown cost: display 'Unknown', not '$0.00'" and "No quota observation:
 * display 'No observation', not '0% remaining'".
 */

export function formatCost(usd: number | null | undefined): string {
  if (usd === null || usd === undefined) return 'Unknown';
  return `$${usd.toFixed(usd < 1 ? 4 : 2)}`;
}

export function formatTokens(n: number | null | undefined): string {
  if (n === null || n === undefined) return 'Unknown';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

export function formatPercent(fraction: number | null | undefined, digits = 0): string {
  if (fraction === null || fraction === undefined || Number.isNaN(fraction)) return 'Unknown';
  return `${(fraction * 100).toFixed(digits)}%`;
}

export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return 'Unknown';
  if (seconds < 60) return `${Math.round(seconds)}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
  return `${(seconds / 3600).toFixed(1)}h`;
}

export function formatCount(n: number | null | undefined): string {
  if (n === null || n === undefined) return 'Unknown';
  return n.toLocaleString();
}

/** "3h 24m" style remaining-time formatting, or null if `iso` is in the
 * past or unparsable (callers should treat null as "no active cooldown",
 * not render a fake value). */
export function formatRemaining(iso: string | null | undefined, now: Date = new Date()): string | null {
  if (!iso) return null;
  const target = new Date(iso);
  if (Number.isNaN(target.getTime())) return null;
  const deltaMs = target.getTime() - now.getTime();
  if (deltaMs <= 0) return null;
  const totalMinutes = Math.floor(deltaMs / 60000);
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m`;
  return '<1m';
}

/** "8m ago" / "3h ago" style age formatting for an observation timestamp. */
export function formatAge(iso: string | null | undefined, now: Date = new Date()): string | null {
  if (!iso) return null;
  const target = new Date(iso);
  if (Number.isNaN(target.getTime())) return null;
  const deltaMs = now.getTime() - target.getTime();
  if (deltaMs < 0) return 'just now';
  const totalMinutes = Math.floor(deltaMs / 60000);
  if (totalMinutes < 1) return 'just now';
  if (totalMinutes < 60) return `${totalMinutes}m ago`;
  const hours = Math.floor(totalMinutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

/** Absolute local-timezone timestamp for an event/observation, e.g.
 * "Jul 9, 3:14 PM" -- the raw ISO string GAH emits is always UTC, which
 * reads as "wrong time" to anyone not in UTC. Returns null for
 * unparsable input so callers can fall back to the raw string rather
 * than rendering "Invalid Date". */
export function formatLocalTime(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit'
  });
}

/** "Updated 3s ago" / "Updated 4m ago" style staleness indicator for a
 * fetch timestamp (epoch ms, as stored on gahStore's Resource.fetchedAt).
 * Finer-grained than formatAge (seconds, not just minutes) since this
 * drives a live "how stale is this panel" readout rather than an
 * observation age buried in a table cell. */
export function formatUpdatedAge(fetchedAt: number | null | undefined, now: number = Date.now()): string {
  if (fetchedAt === null || fetchedAt === undefined) return 'never';
  const deltaMs = now - fetchedAt;
  if (deltaMs < 1000) return 'just now';
  const totalSeconds = Math.floor(deltaMs / 1000);
  if (totalSeconds < 60) return `${totalSeconds}s ago`;
  const totalMinutes = Math.floor(totalSeconds / 60);
  if (totalMinutes < 60) return `${totalMinutes}m ago`;
  const hours = Math.floor(totalMinutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

/** Oldest (most stale) of a set of fetch timestamps, or null if none have
 * ever resolved -- when a page depends on more than one resource, the
 * overall "last updated" should reflect the panel a viewer is least
 * likely to trust, not the freshest one. */
export function oldestFetchedAt(...timestamps: (number | null | undefined)[]): number | null {
  const valid = timestamps.filter((t): t is number => t !== null && t !== undefined);
  return valid.length > 0 ? Math.min(...valid) : null;
}

/** An observation older than this is flagged stale in the UI -- old
 * enough that a human should not trust it as "current." */
export const STALE_THRESHOLD_MS = 30 * 60 * 1000; // 30 minutes

export function isStale(iso: string | null | undefined, now: Date = new Date()): boolean {
  if (!iso) return false; // "no observation" is its own state, not "stale"
  const target = new Date(iso);
  if (Number.isNaN(target.getTime())) return false;
  return now.getTime() - target.getTime() > STALE_THRESHOLD_MS;
}
