import { readFileSync } from 'node:fs';
import type { LedgerEntry, RoleBestFit, RoleMetricsReport, RoleModelMetrics } from '@git-agent-harness/contracts';

/** Ledger modes that are not a model doing a job. */
const NOT_ATTEMPTS = new Set(['review_hold', 'review_hold_release', 'clear_attempts', 'tombstone']);
const APPROVED = /^APPROVE/i;
const SENT_BACK = /^(NEEDS_FIX|REJECT|CHANGES_REQUESTED)/i;

type Cell = RoleModelMetrics & { durations: number[]; blockingFindings: number; keyOf: string };

/** `default`, `auto` and blank mean the backend chose: no model named. */
function modelName(value: string | null | undefined): string | null {
  const trimmed = (value ?? '').trim();
  return trimmed && !/^(default|auto)$/i.test(trimmed) ? trimmed : null;
}

/** A cell is a role on a backend and model; the account instance is recorded, not keyed. */
function key(role: string, backend: string, model: string | null): string {
  return [role, backend, model ?? ''].join('\u0000');
}

/** `7d`, `30d`, `12h` → milliseconds; anything else means no window. */
export function sinceMs(since: string): number | null {
  const match = /^(\d+)([hdw])$/.exec(since.trim());
  if (!match) return null;
  const unit = { h: 3_600_000, d: 86_400_000, w: 7 * 86_400_000 }[match[2] as 'h' | 'd' | 'w'];
  return Number(match[1]) * unit;
}

/** 95% Wilson lower bound: the rate the data supports at least, small samples pulled toward zero. */
export function wilsonLow(successes: number, n: number, z = 1.96): number | null {
  if (n === 0) return null;
  const p = successes / n;
  const denominator = 1 + (z * z) / n;
  const centre = p + (z * z) / (2 * n);
  const spread = z * Math.sqrt((p * (1 - p)) / n + (z * z) / (4 * n * n));
  return Math.max(0, (centre - spread) / denominator);
}

export function confidenceFor(attempts: number): RoleModelMetrics['confidence'] {
  return attempts >= 40 ? 'high' : attempts >= 15 ? 'medium' : attempts >= 5 ? 'low' : 'none';
}

function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function rate(numerator: number, denominator: number): number | null {
  return denominator > 0 ? numerator / denominator : null;
}

function emptyCell(role: string, backend: string, instance: string | null, model: string | null): Cell {
  return {
    role, backend, backend_instance: instance, model, attempts: 0, harness_errors: 0, delivered: 0, delivered_rate: null, delivered_rate_low: null,
    validation_ran: 0, validation_passed: 0, validation_pass_rate: null, reviewed: 0, approved_first_review: 0, first_review_acceptance: null,
    measured: 0, total_tokens: null, tokens_per_attempt: null, tokens_per_delivered: null, total_cost_usd: null, cost_per_delivered_usd: null,
    median_duration_seconds: null, review_verdicts: [], blocking_findings_per_review: null, verdicts_vindicated: 0, verdicts_overturned: 0,
    confidence: 'none', durations: [], blockingFindings: 0, keyOf: key(role, backend, model)
  };
}

/**
 * Per role and model, what the ledger says. A "delivered" attempt produced
 * a pull request that was not sent back by its first review. Review cells
 * are judged by later entries on the same work item: a NEEDS_FIX followed
 * by a passing fix was right; an APPROVE whose work needed a fix afterwards
 * was not.
 */
export function roleMetrics(entries: LedgerEntry[], options: { since?: string; profile?: string | null; now?: number } = {}): RoleMetricsReport {
  const since = options.since ?? '7d';
  const window = sinceMs(since);
  const now = options.now ?? Date.now();
  const inWindow = entries
    .filter((entry) => !options.profile || entry.profile === options.profile)
    .filter((entry) => window === null || now - Date.parse(entry.timestamp) <= window)
    .sort((a, b) => Date.parse(a.timestamp) - Date.parse(b.timestamp));
  const byWork = new Map<string, LedgerEntry[]>();
  for (const entry of inWindow) {
    const id = entry.work_id ?? entry.target_summary ?? '';
    if (id) byWork.set(id, [...(byWork.get(id) ?? []), entry]);
  }
  const cells = new Map<string, Cell>();
  let skipped = 0;
  let harness = 0;
  const cellFor = (entry: LedgerEntry) => {
    const backend = entry.effective_backend || entry.backend || '';
    const instance = entry.usage?.backend_instance ?? null;
    const model = modelName(entry.usage?.actual_model) ?? modelName(entry.effective_model);
    const k = key(entry.mode, backend, model);
    let cell = cells.get(k);
    if (!cell) { cell = emptyCell(entry.mode, backend, instance, model); cells.set(k, cell); }
    if (!cell.backend_instance && instance) cell.backend_instance = instance;
    return cell;
  };
  for (const entry of inWindow) {
    // No backend ran: a hold, a tombstone, or a dispatch the router never placed (`auto`).
    const ran = entry.effective_backend || entry.backend;
    if (NOT_ATTEMPTS.has(entry.mode) || !ran || /^auto$/i.test(ran)) { skipped++; continue; }
    const cell = cellFor(entry);
    if (entry.failure_class === 'harness_error') { cell.harness_errors++; harness++; continue; }
    cell.attempts++;
    if (entry.duration_seconds !== null && entry.duration_seconds !== undefined) cell.durations.push(entry.duration_seconds);
    const tokens = entry.usage?.total_tokens;
    if (typeof tokens === 'number') { cell.measured++; cell.total_tokens = (cell.total_tokens ?? 0) + tokens; }
    const cost = entry.usage?.actual_cost_usd ?? entry.usage?.estimated_cost_usd;
    if (typeof cost === 'number') cell.total_cost_usd = (cell.total_cost_usd ?? 0) + cost;
    const validation = entry.validation_result ?? '';
    if (validation === 'passed' || validation === 'failed') {
      cell.validation_ran++;
      if (validation === 'passed') cell.validation_passed++;
    }
    const later = (byWork.get(entry.work_id ?? entry.target_summary ?? '') ?? []).filter((other) => Date.parse(other.timestamp) > Date.parse(entry.timestamp));
    if (entry.mode === 'review') {
      const verdict = entry.review_verdict ?? entry.validation_result ?? 'none';
      const found = cell.review_verdicts.find(([name]) => name === verdict);
      if (found) found[1]++; else cell.review_verdicts.push([verdict, 1]);
      cell.blockingFindings += Array.isArray((entry as { review_blocking_findings?: unknown[] }).review_blocking_findings) ? ((entry as { review_blocking_findings?: unknown[] }).review_blocking_findings?.length ?? 0) : 0;
      const fixes = later.filter((other) => other.mode === 'fix' && other.failure_class !== 'harness_error');
      if (SENT_BACK.test(verdict) && fixes.some((fix) => fix.validation_result === 'passed' || fix.mr_created)) cell.verdicts_vindicated++;
      if (APPROVED.test(verdict) && fixes.length > 0) cell.verdicts_overturned++;
      // A review that reached a verdict is a delivered review.
      if (verdict !== 'none' && !/^(not_run|deferred)/.test(verdict)) cell.delivered++;
      continue;
    }
    if (entry.mr_created) {
      const firstVerdict = entry.review_verdict ?? later.find((other) => other.mode === 'review' && other.review_verdict)?.review_verdict ?? null;
      if (firstVerdict) {
        cell.reviewed++;
        if (APPROVED.test(firstVerdict) || /^HUMAN_REVIEW/i.test(firstVerdict)) cell.approved_first_review++;
      }
      if (!firstVerdict || !SENT_BACK.test(firstVerdict)) cell.delivered++;
    }
  }
  const finished: RoleModelMetrics[] = [...cells.values()].map((cell) => {
    const { durations, blockingFindings, keyOf: _key, ...rest } = cell;
    const reviews = cell.review_verdicts.reduce((sum, [, count]) => sum + count, 0);
    return {
      ...rest,
      delivered_rate: rate(cell.delivered, cell.attempts),
      delivered_rate_low: wilsonLow(cell.delivered, cell.attempts),
      validation_pass_rate: rate(cell.validation_passed, cell.validation_ran),
      first_review_acceptance: rate(cell.approved_first_review, cell.reviewed),
      tokens_per_attempt: cell.measured > 0 && cell.total_tokens !== null ? cell.total_tokens / cell.measured : null,
      tokens_per_delivered: cell.delivered > 0 && cell.total_tokens !== null && cell.measured === cell.attempts ? cell.total_tokens / cell.delivered : null,
      cost_per_delivered_usd: cell.delivered > 0 && cell.total_cost_usd !== null ? cell.total_cost_usd / cell.delivered : null,
      median_duration_seconds: median(durations),
      blocking_findings_per_review: reviews > 0 ? blockingFindings / reviews : null,
      confidence: confidenceFor(cell.attempts)
    };
  }).sort((a, b) => a.role.localeCompare(b.role) || b.attempts - a.attempts);
  return {
    since, profile: options.profile ?? null, entries: inWindow.length, skipped, harness_errors: harness,
    cells: finished, best_fit: bestFit(finished)
  };
}

/** Per role, models ranked by what their delivered rate is at least (so a
 * lucky 2 for 2 does not beat a steady 30 for 40), then by tokens per
 * delivered PR. Models with too few attempts to judge rank after every
 * model that has enough; models with no attempts are left out. */
export function bestFit(cells: RoleModelMetrics[]): RoleBestFit[] {
  const roles = [...new Set(cells.map((cell) => cell.role))].sort();
  return roles.map((role) => ({
    role,
    ranking: cells.filter((cell) => cell.role === role && cell.attempts > 0)
      .map((cell) => ({ backend: cell.backend, backend_instance: cell.backend_instance, model: cell.model, score: cell.delivered_rate_low ?? 0, attempts: cell.attempts, confidence: cell.confidence, tokens: cell.tokens_per_delivered ?? Infinity }))
      .sort((a, b) => Number(b.confidence !== 'none') - Number(a.confidence !== 'none') || b.score - a.score || a.tokens - b.tokens || b.attempts - a.attempts)
      .map(({ tokens: _tokens, ...entry }) => entry)
  }));
}

/** The ledger file as entries; lines that are not JSON are skipped. */
export function readLedger(path: string): LedgerEntry[] {
  const entries: LedgerEntry[] = [];
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    if (!line.trim()) continue;
    try { entries.push(JSON.parse(line) as LedgerEntry); } catch { /* A half-written or foreign line. */ }
  }
  return entries;
}
