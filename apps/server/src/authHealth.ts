/**
 * Issue #1271: expired provider logins are noticed and reported instead of
 * discovered mid-task.
 *
 * Every node runs an AuthHealthProber: `gah auth-health` on start, every 30
 * minutes, and on demand. Its latest report rides in the node's /api/status,
 * so central sees each worker's logins in the observation it already polls.
 *
 * Central runs one AuthHealthMonitor. It merges its own report, the workers'
 * reports, and chat turns that failed to authenticate, and announces each
 * login's transition to expired or back to ok exactly once.
 */
import crypto from 'node:crypto';
import type { ActivityEvent, AuthHealthRow, AuthProbe, AuthState, NodeAuthHealth, NodeObservationSnapshot } from '@git-agent-harness/contracts';
import { classifyAuthFailure, runAuthHealth } from './gahCli.js';

const PROBE_INTERVAL_MS = 30 * 60 * 1000;
const AUTH_STATES: ReadonlySet<AuthState> = new Set(['ok', 'expired', 'missing', 'unknown', 'error']);
const bad = (state: AuthState | undefined) => state === 'expired' || state === 'missing';
const loginKey = (probe: Pick<AuthProbe, 'backend' | 'provider'>) => `${probe.backend}|${probe.provider ?? ''}`;

/** Accepts only the documented shape from a worker; drops anything else. */
export function parseNodeAuthHealth(value: unknown): NodeAuthHealth | null {
  if (!value || typeof value !== 'object') return null;
  const record = value as Record<string, unknown>;
  if (typeof record.checked_at !== 'string' || !Array.isArray(record.probes)) return null;
  const text = (item: unknown, max: number) => typeof item === 'string' && item.length > 0 && item.length <= max;
  const probes = record.probes.flatMap((item): AuthProbe[] => {
    if (!item || typeof item !== 'object') return [];
    const probe = item as Record<string, unknown>;
    if (!text(probe.backend, 64) || !(probe.provider === null || text(probe.provider, 64))
      || !AUTH_STATES.has(probe.state as AuthState)
      || !['probe', 'dispatch', 'chat'].includes(probe.source as string)) return [];
    return [{
      backend: probe.backend as string,
      provider: probe.provider as string | null,
      state: probe.state as AuthState,
      ...(text(probe.detail, 200) ? { detail: probe.detail as string } : {}),
      source: probe.source as AuthProbe['source'],
      ...(text(probe.since, 64) ? { since: probe.since as string } : {})
    }];
  });
  return { checked_at: record.checked_at, probes: probes.slice(0, 64) };
}

/** Runs this node's login checks and remembers when each login entered its
 * current state, so repeated checks of an unchanged login stay one event. */
export class AuthHealthProber {
  private report: NodeAuthHealth | null = null;
  private since = new Map<string, { state: AuthState; since: string }>();
  private running: Promise<NodeAuthHealth | null> | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;
  private listeners = new Set<(report: NodeAuthHealth) => void>();

  constructor(
    private readonly run: () => Promise<{ checked_at: string; probes: Omit<AuthProbe, 'since'>[] }> = runAuthHealth,
    private readonly intervalMs = PROBE_INTERVAL_MS
  ) {}

  start(): void {
    void this.refresh();
    this.timer = setInterval(() => void this.refresh(), this.intervalMs);
    this.timer.unref?.();
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  latest(): NodeAuthHealth | null {
    return this.report;
  }

  onUpdate(listener: (report: NodeAuthHealth) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  /** Concurrent callers share one run. A failed run keeps the last report. */
  refresh(): Promise<NodeAuthHealth | null> {
    this.running ??= this.run()
      .then((raw) => {
        const parsed = parseNodeAuthHealth(raw);
        if (!parsed) throw new Error('gah auth-health returned an unexpected shape');
        const next = new Map<string, { state: AuthState; since: string }>();
        const probes = parsed.probes.map((probe) => {
          const previous = this.since.get(loginKey(probe));
          const entry = previous?.state === probe.state ? previous : { state: probe.state, since: parsed.checked_at };
          next.set(loginKey(probe), entry);
          return { ...probe, since: entry.since };
        });
        this.since = next;
        this.report = { checked_at: parsed.checked_at, probes };
        for (const listener of this.listeners) listener(this.report);
        return this.report;
      })
      .catch((error) => {
        console.error(`[auth] login check failed: ${error instanceof Error ? error.message : String(error)}`);
        return this.report;
      })
      .finally(() => { this.running = null; });
    return this.running;
  }
}

export interface AuthTransition {
  event: ActivityEvent;
}

type NodeReport = { nodeId: string; nodeName: string; health: NodeAuthHealth | null };

/** Central's fleet view of logins, and the one place that decides when a
 * login expired or came back. */
export class AuthHealthMonitor {
  /** Chat turns that failed to authenticate, keyed by node then backend. */
  private chatFailures = new Map<string, Map<string, string>>();
  private lastProbeState = new Map<string, AuthState>();
  private lastState = new Map<string, AuthState>();
  private current: AuthHealthRow[] = [];
  private listeners = new Set<(transition: AuthTransition) => void>();

  constructor(
    private readonly local: { nodeId: string; nodeName: string; prober: AuthHealthProber },
    private readonly workers: () => NodeObservationSnapshot[],
    private readonly classify: (backend: string, text: string) => Promise<boolean> = classifyAuthFailure,
    private readonly now: () => number = Date.now
  ) {
    local.prober.onUpdate(() => this.recompute());
  }

  rows(): AuthHealthRow[] {
    return this.current;
  }

  onTransition(listener: (transition: AuthTransition) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  /** Worker observations changed; re-read their reports. */
  observationsChanged(): void {
    this.recompute();
  }

  /** Called for every chat turn attempt on any node. A failure that reads as
   * an authentication failure marks that login expired at once; the next
   * successful turn on the same node and backend clears it. */
  async turnFinished(nodeId: string, backend: string, failure?: string): Promise<void> {
    const failures = this.chatFailures.get(nodeId);
    if (failure === undefined) {
      const key = `${nodeId}|${loginKey({ backend, provider: null })}`;
      const failed = this.current.find((row) => `${row.node_id}|${loginKey(row)}` === key && bad(row.state));
      if (!failures?.delete(backend)) return;
      this.recompute();
      // No check covers this login, so its row is gone; the good turn is the restore.
      if (failed && !this.lastState.has(key)) this.emit(authActivity({ ...failed, state: 'ok', since: new Date(this.now()).toISOString() }, 'auth_restored', this.now()));
      return;
    }
    if (!await this.classify(backend, failure)) return;
    const next = this.chatFailures.get(nodeId) ?? new Map<string, string>();
    if (next.has(backend)) return;
    next.set(backend, new Date(this.now()).toISOString());
    this.chatFailures.set(nodeId, next);
    this.recompute();
    if (nodeId === this.local.nodeId) void this.local.prober.refresh();
  }

  private reports(): NodeReport[] {
    const reports: NodeReport[] = [
      { nodeId: this.local.nodeId, nodeName: this.local.nodeName, health: this.local.prober.latest() },
      ...this.workers()
        .filter((observation) => observation.node_id !== this.local.nodeId)
        .map((observation) => ({ nodeId: observation.node_id, nodeName: observation.display_name, health: observation.auth_health ?? null }))
    ];
    // A chat can fail on a worker central has not observed yet.
    for (const nodeId of this.chatFailures.keys()) {
      if (!reports.some((report) => report.nodeId === nodeId)) reports.push({ nodeId, nodeName: nodeId, health: null });
    }
    return reports;
  }

  private recompute(): void {
    const rows: AuthHealthRow[] = [];
    for (const { nodeId, nodeName, health } of this.reports()) {
      const failures = this.chatFailures.get(nodeId);
      const byKey = new Map<string, AuthHealthRow>();
      for (const probe of health?.probes ?? []) {
        const key = `${nodeId}|${loginKey(probe)}`;
        // Logging in again (the node's check turns ok) clears a chat failure.
        if (probe.source === 'probe' && probe.state === 'ok' && bad(this.lastProbeState.get(key))) failures?.delete(probe.backend);
        this.lastProbeState.set(key, probe.state);
        const existing = byKey.get(loginKey(probe));
        if (!existing || (bad(probe.state) && !bad(existing.state))) byKey.set(loginKey(probe), { ...probe, node_id: nodeId, node_name: nodeName });
      }
      for (const [backend, since] of failures ?? []) {
        byKey.set(loginKey({ backend, provider: null }), {
          backend, provider: null, state: 'expired', source: 'chat',
          detail: 'A chat turn failed to authenticate.', since, node_id: nodeId, node_name: nodeName
        });
      }
      rows.push(...byKey.values());
    }
    this.current = rows;
    this.announce(rows);
  }

  private announce(rows: AuthHealthRow[]): void {
    const seen = new Set<string>();
    for (const row of rows) {
      const key = `${row.node_id}|${loginKey(row)}`;
      seen.add(key);
      const previous = this.lastState.get(key);
      if (row.state !== 'ok' && !bad(row.state)) continue;
      this.lastState.set(key, row.state);
      // A CLI that was never logged in is shown, not pushed; a login that
      // worked and stopped, or one that says it expired, is pushed.
      const expired = bad(row.state) && (previous === 'ok' || (previous === undefined && row.state === 'expired'));
      const restored = row.state === 'ok' && bad(previous);
      if (!expired && !restored) continue;
      this.emit(authActivity(row, expired ? 'auth_expired' : 'auth_restored', this.now()));
    }
    for (const key of this.lastState.keys()) if (!seen.has(key)) this.lastState.delete(key);
  }

  private emit(event: ActivityEvent): void {
    for (const listener of this.listeners) listener({ event });
  }
}

export function authActivity(row: AuthHealthRow, kind: 'auth_expired' | 'auth_restored', now: number): ActivityEvent {
  const since = row.since ?? new Date(now).toISOString();
  const login = [row.node_name, row.backend, row.provider].filter(Boolean).join(' · ');
  return {
    id: `auth:${crypto.createHash('sha256').update(JSON.stringify([row.node_id, row.backend, row.provider, kind, since])).digest('hex').slice(0, 24)}`,
    occurredAt: since,
    profile: null,
    kind,
    severity: kind === 'auth_expired' ? 'error' : 'success',
    title: kind === 'auth_expired' ? `${login}: login expired` : `${login}: login restored`,
    message: kind === 'auth_expired'
      ? `${row.detail ?? 'The login no longer works.'} Log in again on ${row.node_name}.`
      : 'The login works again.',
    nodeId: row.node_id
  };
}

let chatMonitor: AuthHealthMonitor | null = null;

/** Central's chat turns report to this monitor; unset on workers and in tests. */
export function configureChatAuthHealth(monitor: AuthHealthMonitor | null): void {
  chatMonitor = monitor;
}

/** Records one chat turn attempt's outcome for login health. Never throws. */
export function reportChatTurn(nodeId: string | undefined, backend: string, failure?: string): void {
  if (!chatMonitor || !nodeId) return;
  chatMonitor.turnFinished(nodeId, backend, failure).catch((error) => {
    console.error(`[auth] could not classify a chat failure: ${error instanceof Error ? error.message : String(error)}`);
  });
}
