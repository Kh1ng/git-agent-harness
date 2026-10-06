import type { ActiveClaim, ControllerActivity, DeviceAgent, QuotaCandidateStatus, QuotaCheck, QuotaObservation, QuotaSnapshot, RecentLedgerSummary, Session } from '@git-agent-harness/contracts';

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
  /** The account's quota pool this window belongs to (Gemini, External models, Vibe…). */
  pool: string | null;
}

/** A subscription: everything the quota snapshot knows about one account. */
export interface SubscriptionUsage {
  /** The account: its named credential, or the backend instance without its pool suffix. */
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
  if (/(^|[-_ ])monthly$/.test(lower)) return { label: 'Monthly', windowMs: 30 * DAY };
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

/** `agy-second:external` -> `agy-second`; a named credential wins. */
export function accountId(source: { backend: string; backend_instance?: string | null; credential_id?: string | null }): string {
  return source.credential_id ?? (source.backend_instance ?? source.backend).split(':')[0];
}

const POOL_LABELS: Record<string, string> = {
  external: 'External models', 'google-native': 'Gemini', gemini: 'Gemini',
  vibe: 'Vibe', 'vibe-monthly': 'Vibe', 'vibe-code-included-monthly': 'Vibe', api: 'API', admin: 'API'
};

/** `agy-second:external` -> `External models`; null when the scope names no pool. */
function poolLabel(source: { backend_instance?: string | null; quota_pool?: string | null }, account: string): string | null {
  const raw = [source.quota_pool, source.backend_instance]
    .map((value) => value?.split(':').slice(1).join(':') || (value && value !== account ? value : null))
    .find(Boolean);
  if (!raw) return null;
  return POOL_LABELS[raw.toLowerCase()] ?? raw.replace(/[-_]+/g, ' ').replace(/^\w/, (c) => c.toUpperCase());
}

function toWindow(observation: QuotaObservation, index: number, pool: string | null): UsageWindow {
  const { label, windowMs } = parseWindow(observation.quota_window);
  const base = observation.model && observation.model !== label ? `${label} · ${observation.model}` : label;
  return {
    key: `${pool ?? ''}-${observation.quota_window ?? 'usage'}-${observation.model ?? ''}-${index}`,
    label: pool ? `${pool} · ${base}` : base,
    usedPercent: usedPercent(observation),
    resetAt: observation.quota_reset_at ?? null,
    windowMs,
    observedAt: observation.observed_at ?? null,
    model: observation.model ?? null,
    pool
  };
}

type Scope = Pick<QuotaCheck, 'backend' | 'provider' | 'backend_instance' | 'credential_id' | 'quota_pool' | 'quota_observations'>;

/**
 * One entry per account, not per routing candidate or model (#1411): an
 * account's pools (Antigravity's Gemini and external-model pools, Mistral's
 * API and Vibe pools) are windows of the same ring. Router aliases
 * (`cli-router…`) route onto those accounts and are not accounts themselves.
 */
export function subscriptionUsage(snapshot: Partial<Pick<QuotaSnapshot, 'candidates' | 'quota_checks'>> | null | undefined): SubscriptionUsage[] {
  const candidates = snapshot?.candidates ?? [];
  const scopes: Scope[] = [...candidates, ...(snapshot?.quota_checks ?? [])];
  const accounts = new Map<string, { scopes: Scope[]; candidates: QuotaCandidateStatus[] }>();
  for (const scope of scopes) {
    const id = accountId(scope);
    if (id.startsWith('cli-router')) continue;
    const entry = accounts.get(id) ?? { scopes: [], candidates: [] };
    entry.scopes.push(scope);
    if (candidates.includes(scope as QuotaCandidateStatus)) entry.candidates.push(scope as QuotaCandidateStatus);
    accounts.set(id, entry);
  }
  return [...accounts.entries()].map(([id, account]) => {
    const pools = new Set(account.scopes.map((scope) => poolLabel(scope, id)).filter(Boolean));
    const byWindow = new Map<string, UsageWindow>();
    account.scopes.forEach((scope) => (scope.quota_observations ?? []).forEach((observation, index) => {
      const window = toWindow(observation, index, pools.size > 1 ? poolLabel(scope, id) : null);
      const key = `${window.pool}|${observation.quota_window}|${window.model}`;
      const previous = byWindow.get(key);
      // A candidate and its account check report the same reading; keep the newest.
      if (!previous || (window.observedAt ?? '') > (previous.observedAt ?? '')) byWindow.set(key, window);
    }));
    // An account-wide reading repeated under each pool is shown once, per pool.
    const pooled = new Set([...byWindow.values()].filter((window) => window.pool).map((window) => window.windowMs ?? window.label));
    const windows = [...byWindow.values()].filter((window) => window.pool || !pooled.has(window.windowMs ?? window.label))
      // Short windows first, then by name, so the ring's outer arc is the session.
      .sort((a, b) => (a.windowMs ?? Infinity) - (b.windowMs ?? Infinity) || a.label.localeCompare(b.label));
    const measured = windows.filter((window) => window.usedPercent !== null);
    const tightest = measured.length ? measured.reduce((max, window) => (window.usedPercent! > max.usedPercent! ? window : max)) : null;
    const routed = account.candidates;
    const blocked = routed.length > 0 && routed.every((candidate) => !candidate.eligible_now) ? routed[0] : null;
    const first = account.scopes[0];
    const provider = account.scopes.map((scope) => scope.provider).find(Boolean) ?? null;
    const costs = routed.map((candidate) => candidate.usage?.actual_cost_usd).filter((cost): cost is number => typeof cost === 'number');
    return {
      id,
      backend: first.backend,
      provider,
      providerLabel: providerLabel(provider, first.backend),
      model: null,
      windows,
      tightest,
      eligible: !blocked,
      reason: blocked?.reason ?? null,
      unavailableUntil: blocked?.unavailable_until ?? null,
      costUsd: costs.length ? costs.reduce((sum, cost) => sum + cost, 0) : null,
      observedAt: windows.map((window) => window.observedAt).filter((value): value is string => !!value).sort().at(-1)
        ?? routed.map((candidate) => candidate.observed_at).filter((value): value is string => !!value).sort().at(-1) ?? null
    };
  }).sort((a, b) => a.providerLabel.localeCompare(b.providerLabel) || a.id.localeCompare(b.id));
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
    input.subscriptions.find((usage) => instance && usage.id === accountId({ backend: backend ?? '', backend_instance: instance }))
      ?? input.subscriptions.find((usage) => usage.backend === backend);
  const busy = new Set<string>();
  for (const session of input.sessions) {
    if (!['starting', 'running', 'stopping'].includes(session.status)) continue;
    const match = byBackend(session.backend ?? session.providerKind, session.instanceId);
    if (match) busy.add(match.id);
  }
  for (const agent of input.factoryAgents ?? []) {
    // One CLI can draw on several subscriptions: Antigravity bills Gemini and Claude
    // models to separate allowances. The model the process was started with says which.
    const match = input.subscriptions.find((usage) => usage.backend === agent.tool && !!agent.model && usage.model === agent.model)
      ?? byBackend(agent.tool);
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
