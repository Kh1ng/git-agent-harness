import type { ActiveClaim, ControllerActivity, DeviceAgent, QuotaCandidateStatus, QuotaObservation, QuotaSnapshot, RecentLedgerSummary, Session } from '@git-agent-harness/contracts';

/** One rate-limit window of a subscription, as the provider reports it. */
export interface UsageWindow {
  key: string;
  /** Session (5h), Weekly, Daily, or the provider's own name for it. */
  label: string;
  usedPercent: number | null;
  resetAt: string | null;
  /** Length of the window when its name says so (5h, weekly, 10080m). */
  windowMs: number | null;
  observedAt: string | null;
  /** The model this window is for, when the provider splits by model. */
  model: string | null;
}

/** A subscription: everything the quota snapshot knows about one account. */
export interface SubscriptionUsage {
  id: string;
  backend: string;
  /** Subscription provider: openai, anthropic, antigravity, z-ai… */
  provider: string | null;
  providerLabel: string;
  model: string | null;
  windows: UsageWindow[];
  /** The window closest to its limit. */
  tightest: UsageWindow | null;
  eligible: boolean;
  reason: string | null;
  unavailableUntil: string | null;
  /** Spend this period when the provider reports dollars rather than percent. */
  costUsd: number | null;
  observedAt: string | null;
}

const PROVIDER_LABELS: Record<string, string> = {
  openai: 'OpenAI', anthropic: 'Anthropic', antigravity: 'Antigravity', 'z-ai': 'Z.ai', google: 'Google',
  mistral: 'Mistral', nous: 'Nous', github: 'GitHub', 'github-copilot': 'GitHub Copilot'
};

export function providerLabel(provider: string | null | undefined, backend: string): string {
  const key = (provider ?? backend).toLowerCase();
  return PROVIDER_LABELS[key] ?? key.charAt(0).toUpperCase() + key.slice(1);
}

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

/** Turn a provider's window name into a label and a length, when the name carries one. */
export function parseWindow(name: string | null | undefined): { label: string; windowMs: number | null } {
  const raw = (name ?? '').trim();
  const lower = raw.toLowerCase();
  if (!raw) return { label: 'Usage', windowMs: null };
  if (/^(weekly|7 ?d(ays?)?|10080 ?m(in)?)$/.test(lower)) return { label: 'Weekly', windowMs: 7 * DAY };
  if (/^(daily|1 ?d(ay)?|24 ?h(ours?)?|1440 ?m(in)?)$/.test(lower)) return { label: 'Daily', windowMs: DAY };
  if (/^monthly$/.test(lower)) return { label: 'Monthly', windowMs: 30 * DAY };
  const hours = /^(\d+)[ -]?h(ours?)?$/.exec(lower);
  if (hours) return { label: `Session (${hours[1]}h)`, windowMs: Number(hours[1]) * HOUR };
  const minutes = /^(\d+) ?m(in(utes?)?)?$/.exec(lower);
  if (minutes) {
    const count = Number(minutes[1]);
    if (count % 1440 === 0) return { label: count === 1440 ? 'Daily' : `${count / 1440}-day`, windowMs: count * 60_000 };
    if (count % 60 === 0) return { label: `Session (${count / 60}h)`, windowMs: count * 60_000 };
    return { label: `${count} min`, windowMs: count * 60_000 };
  }
  return { label: raw, windowMs: null };
}

function usedPercent(observation: QuotaObservation): number | null {
  const remaining = observation.quota_remaining_percent;
  if (typeof remaining === 'number' && Number.isFinite(remaining)) return 100 - Math.min(100, Math.max(0, remaining));
  return null;
}

function toWindow(observation: QuotaObservation, index: number): UsageWindow {
  const { label, windowMs } = parseWindow(observation.quota_window);
  return {
    key: `${observation.quota_window ?? 'usage'}-${observation.model ?? ''}-${index}`,
    label: observation.model && observation.model !== label ? `${label} · ${observation.model}` : label,
    usedPercent: usedPercent(observation),
    resetAt: observation.quota_reset_at ?? null,
    windowMs,
    observedAt: observation.observed_at ?? null,
    model: observation.model ?? null
  };
}

function fromCandidate(candidate: QuotaCandidateStatus): SubscriptionUsage {
  const windows = (candidate.quota_observations ?? []).map(toWindow)
    // Short windows first, then by name, so the ring's outer arc is the session.
    .sort((a, b) => (a.windowMs ?? Infinity) - (b.windowMs ?? Infinity) || a.label.localeCompare(b.label));
  const measured = windows.filter((window) => window.usedPercent !== null);
  const tightest = measured.length ? measured.reduce((max, window) => (window.usedPercent! > max.usedPercent! ? window : max)) : null;
  const id = candidate.backend_instance ?? candidate.backend;
  return {
    id,
    backend: candidate.backend,
    provider: candidate.provider ?? null,
    providerLabel: providerLabel(candidate.provider, candidate.backend),
    model: candidate.model,
    windows,
    tightest,
    eligible: candidate.eligible_now,
    reason: candidate.reason ?? null,
    unavailableUntil: candidate.unavailable_until ?? null,
    costUsd: candidate.usage?.actual_cost_usd ?? null,
    observedAt: windows.map((window) => window.observedAt).filter((value): value is string => !!value).sort().at(-1) ?? candidate.observed_at ?? null
  };
}

/** One entry per routing candidate; an instance shared by several candidates appears once. */
export function subscriptionUsage(snapshot: Pick<QuotaSnapshot, 'candidates'> | null | undefined): SubscriptionUsage[] {
  const seen = new Map<string, SubscriptionUsage>();
  for (const candidate of snapshot?.candidates ?? []) {
    const usage = fromCandidate(candidate);
    const existing = seen.get(usage.id);
    if (!existing) { seen.set(usage.id, usage); continue; }
    if (existing.windows.length === 0 && usage.windows.length > 0) seen.set(usage.id, { ...usage, model: existing.model });
  }
  return [...seen.values()];
}

/**
 * The subscriptions with a job running now: a live dashboard session names
 * its backend; a running controller run or an active claim is credited to
 * the backend of the most recent dispatch when it is that same work item.
 */
export function busySubscriptionIds(input: {
  subscriptions: SubscriptionUsage[];
  sessions: Session[];
  controllerRuns: ControllerActivity[];
  claims: ActiveClaim[];
  recentLedger: RecentLedgerSummary | null | undefined;
  /** Agent processes in factory worktrees: each is a subscription at work. */
  factoryAgents?: DeviceAgent[];
}): Set<string> {
  const byBackend = (backend: string | null | undefined, instance?: string | null) =>
    input.subscriptions.find((usage) => instance && usage.id === instance) ?? input.subscriptions.find((usage) => usage.backend === backend);
  const busy = new Set<string>();
  for (const session of input.sessions) {
    if (!['starting', 'running', 'stopping'].includes(session.status)) continue;
    const match = byBackend(session.backend ?? session.providerKind, session.instanceId);
    if (match) busy.add(match.id);
  }
  for (const agent of input.factoryAgents ?? []) {
    const match = byBackend(agent.tool);
    if (match) busy.add(match.id);
  }
  const recent = input.recentLedger;
  if (recent?.most_recent_work_id) {
    const inFlight = input.controllerRuns.some((run) => run.status === 'running' && run.work_id === recent.most_recent_work_id)
      || input.claims.some((claim) => claim.work_id === recent.most_recent_work_id);
    const match = inFlight ? byBackend(recent.most_recent_effective_backend) : null;
    if (match) busy.add(match.id);
  }
  return busy;
}

/** Where the window will be at reset if use keeps its pace so far: used%
 * scaled from the elapsed part of the window to all of it. Null until a
 * meaningful slice of the window has passed. */
export function projectedPercent(window: UsageWindow, now: number): number | null {
  if (window.usedPercent === null || !window.windowMs || !window.resetAt) return null;
  const remaining = Date.parse(window.resetAt) - now;
  if (!Number.isFinite(remaining) || remaining < 0 || remaining > window.windowMs) return null;
  const elapsed = window.windowMs - remaining;
  // The first tenth of a window says little about the pace; a fresh session would read 100%.
  if (elapsed < window.windowMs * 0.1) return null;
  return Math.min(100, Math.round((window.usedPercent * window.windowMs) / elapsed));
}

/** `20 min`, `2d 14h`, `3h 05m`, or `now`. */
export function formatUntil(iso: string | null, now: number): string | null {
  if (!iso) return null;
  const ms = Date.parse(iso) - now;
  if (!Number.isFinite(ms)) return null;
  if (ms <= 0) return 'now';
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${String(minutes % 60).padStart(2, '0')}m`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}

/** Tone for a used percentage: fine, getting close, or nearly out. */
export function usageTone(percent: number | null): 'good' | 'warning' | 'critical' | 'unknown' {
  if (percent === null) return 'unknown';
  if (percent >= 90) return 'critical';
  if (percent >= 70) return 'warning';
  return 'good';
}
