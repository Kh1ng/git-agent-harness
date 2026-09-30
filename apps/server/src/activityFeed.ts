import crypto from 'node:crypto';
import { appendFileSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { notifiableActivity, type ActivityEvent, type ControllerEvent, type DeliveryReceipt, type GatewayHealthSummary, type QuotaSnapshot } from '@git-agent-harness/contracts';
import { controllerDispatchSucceeded } from './controllerActivity.js';
import { redactTextSecrets } from './managerChat/redactText.js';

/** Routine events kept; notified events within the retention window don't count. */
const MAX_STORED_EVENTS = 2_000;
const NOTIFIED_RETENTION_MS = 30 * 24 * 60 * 60 * 1000;
const REPLAY_LIMIT = 200;

/** Sends one recorded event through every delivery method and reports what
 * happened to each target. */
export type ActivityDeliverer = (event: ActivityEvent) => Promise<DeliveryReceipt[] | void> | DeliveryReceipt[] | void;

export type ActivityFeedChange =
  | { kind: 'updated'; event: ActivityEvent }
  | { kind: 'unread'; count: number };

export type ChatLifecycleEvent = {
  phase: 'start' | 'tool' | 'permission' | 'end';
  profile: string;
  sessionId?: string;
  turn: number;
  occurredAt: string;
  backend?: string | null;
  model?: string | null;
  tool?: string;
  permissionId?: string;
  outcome?: 'complete' | 'error' | 'cancelled';
  reply?: string;
  error?: string;
};

function stableId(prefix: string, value: unknown): string {
  return `${prefix}:${crypto.createHash('sha256').update(JSON.stringify(value)).digest('hex').slice(0, 24)}`;
}

function collapsedPreview(value: string, max: number): string {
  return redactTextSecrets(value).replace(/\s+/g, ' ').trim().slice(0, max);
}

/** A tool title can carry a full command line; only its leading name may leave the server. */
export function chatToolName(value?: string): string | null {
  return value?.match(/^[\p{L}\p{N}_-]{1,40}/u)?.[0] ?? null;
}

function permissionTool(value?: string): string {
  const name = chatToolName(value);
  return name ? `${name} requested` : 'Agent tool requested';
}

/** Convert live chat state into the same durable operator feed as dispatch. */
export function activityFromChat(event: ChatLifecycleEvent): ActivityEvent | null {
  const sessionId = event.sessionId ?? 'default';
  const shared = {
    occurredAt: event.occurredAt,
    profile: event.profile,
    sessionId
  };
  if (event.phase === 'permission') {
    return {
      ...shared,
      id: stableId('chat_permission', {
        profile: event.profile,
        sessionId,
        turn: event.turn,
        request: event.permissionId ?? event.occurredAt
      }),
      kind: 'chat_permission_requested',
      severity: 'warning',
      title: `${event.profile}: permission required`,
      message: permissionTool(event.tool)
    };
  }
  if (event.phase !== 'end' || event.outcome === 'cancelled') return null;
  if (event.outcome === 'complete') {
    return {
      ...shared,
      id: `chat:${event.profile}:${sessionId}:${event.turn}:chat_turn_completed`,
      kind: 'chat_turn_completed',
      severity: 'success',
      title: `${event.profile}: reply ready`,
      message: collapsedPreview(event.reply ?? '', 120) || 'The agent finished its reply.'
    };
  }
  return {
    ...shared,
    id: `chat:${event.profile}:${sessionId}:${event.turn}:chat_turn_failed`,
    kind: 'chat_turn_failed',
    severity: 'error',
    title: `${event.profile}: chat failed`,
    message: collapsedPreview(event.error ?? 'The chat turn failed.', 200)
  };
}

/** Convert the existing durable controller log into the small operator feed. */
export function activityFromController(event: ControllerEvent): ActivityEvent | null {
  const profile = event.profile ?? null;
  const shared = {
    id: stableId('controller', event),
    origin: 'controller' as const,
    occurredAt: event.timestamp,
    profile,
    message: event.details,
    workId: event.work_id ?? null
  };
  const lower = event.details.toLowerCase();

  if (lower.includes('gateway') && (event.event_type === 'human_required' || event.event_type === 'dispatch_finished')) {
    return { ...shared, kind: 'gateway_down', severity: 'error', title: 'Memory gateway unavailable' };
  }
  if (event.event_type === 'backend_marked_unavailable' && lower.includes('quota')) {
    return { ...shared, kind: 'quota_near_limit', severity: 'warning', title: 'Quota is constraining work' };
  }
  if (event.event_type === 'human_required' || event.event_type === 'review_budget_exhausted') {
    return { ...shared, kind: 'action_required', severity: 'warning', title: 'Operator action required' };
  }
  if (event.event_type !== 'dispatch_finished') return null;

  const succeeded = controllerDispatchSucceeded(event.details);
  if (succeeded && /^review_mr\s*:/i.test(event.details)) {
    return { ...shared, kind: 'review_ready', severity: 'success', title: 'Review ready' };
  }
  return succeeded
    ? { ...shared, kind: 'dispatch_completed', severity: 'success', title: 'Work finished' }
    : { ...shared, kind: 'dispatch_failed', severity: 'error', title: 'Work failed' };
}

export function activityFromNode(transition: {
  nodeId: string;
  displayName: string;
  state: 'offline' | 'back';
  occurredAt: string;
  message: string;
}): ActivityEvent {
  return {
    id: stableId('node', transition),
    occurredAt: transition.occurredAt,
    profile: null,
    kind: transition.state === 'back' ? 'node_back' : 'node_offline',
    severity: transition.state === 'back' ? 'success' : 'error',
    title: transition.state === 'back'
      ? `${transition.displayName} is back online`
      : `${transition.displayName} is offline`,
    message: transition.message,
    nodeId: transition.nodeId
  };
}

export function activitiesFromQuota(snapshot: QuotaSnapshot): ActivityEvent[] {
  const events: ActivityEvent[] = [];
  for (const candidate of snapshot.candidates) {
    for (const observation of candidate.quota_observations ?? []) {
      const remaining = observation.quota_remaining_percent;
      if (remaining === null || remaining === undefined || remaining > 10) continue;
      const identity = `${candidate.backend}${candidate.model ? `/${candidate.model}` : ''}`;
      const occurredAt = observation.observed_at ?? snapshot.generated_at;
      events.push({
        id: stableId('quota', { profile: snapshot.profile.profile, identity, remaining, occurredAt }),
        occurredAt,
        profile: snapshot.profile.profile,
        kind: 'quota_near_limit',
        severity: 'warning',
        title: `${identity} quota is near its limit`,
        message: `${remaining.toFixed(1)}% remains${observation.quota_reset_at ? `; resets ${observation.quota_reset_at}` : ''}.`
      });
    }
  }
  return events;
}

export function activityFromGateway(health: GatewayHealthSummary): ActivityEvent | null {
  if (!health.degraded || health.lastFailedAt === null) return null;
  const occurredAt = new Date(health.lastFailedAt).toISOString();
  return {
    id: stableId('gateway', { occurredAt, error: health.lastError }),
    occurredAt,
    profile: null,
    kind: 'gateway_down',
    severity: 'error',
    title: 'Memory gateway unavailable',
    message: health.lastError ?? 'The last memory gateway request failed.'
  };
}

/** The durable operator feed. It owns retention, per-notification read state,
 * and delivery receipts; callers record events and listen for changes. */
export class ActivityFeed {
  private events: ActivityEvent[] = [];
  private ids = new Set<string>();
  private listeners = new Set<(change: ActivityFeedChange) => void>();

  constructor(
    private path: string | null = process.env.GAH_ACTIVITY_PATH ?? resolve(process.cwd(), 'config/activity.jsonl'),
    private deliver?: ActivityDeliverer,
    private now: () => number = Date.now
  ) {
    if (!path || !existsSync(path)) return;
    try {
      for (const line of readFileSync(path, 'utf8').split('\n')) {
        if (!line.trim()) continue;
        const event = JSON.parse(line) as ActivityEvent;
        if (event?.id && !this.ids.has(event.id)) {
          this.events.push(event);
          this.ids.add(event.id);
        }
      }
      this.trim();
    } catch (error) {
      console.error(`Failed to read activity feed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  onChange(listener: (change: ActivityFeedChange) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  /** A delivered notification starts unread; a silently backfilled one never
   * reached anyone, so it starts read. */
  record(event: ActivityEvent, deliver = true): boolean {
    if (this.ids.has(event.id)) return false;
    if (notifiableActivity(event)) event.readAt = deliver ? null : new Date(this.now()).toISOString();
    this.events.push(event);
    this.ids.add(event.id);
    if (this.path) {
      mkdirSync(dirname(this.path), { recursive: true });
      appendFileSync(this.path, `${JSON.stringify(event)}\n`, { mode: 0o600 });
    }
    if (this.events.length > MAX_STORED_EVENTS && this.trim()) this.persist();
    if (event.readAt === null) this.emit({ kind: 'unread', count: this.unreadCount() });
    if (deliver && this.deliver) {
      void Promise.resolve()
        .then(() => this.deliver!(event))
        .then((receipts) => this.recordDeliveries(event.id, receipts ?? []))
        .catch((error) => {
          console.error(`[activity] delivery failed for ${event.id}: ${error instanceof Error ? error.message : String(error)}`);
        });
    }
    return true;
  }

  replay(profile: string, cursor?: string): ActivityEvent[] {
    const relevant = this.events.filter((event) => event.profile === null || event.profile === profile);
    const cursorIndex = cursor ? relevant.findIndex((event) => event.id === cursor) : -1;
    return (cursorIndex >= 0 ? relevant.slice(cursorIndex + 1) : relevant.slice(-REPLAY_LIMIT));
  }

  /** Every retained event that passed the wake filter, across profiles, newest first. */
  notifications(): ActivityEvent[] {
    // Recording order is not event order: controller history can arrive late.
    return this.events.filter(notifiableActivity).sort((a, b) => Date.parse(b.occurredAt) - Date.parse(a.occurredAt));
  }

  unreadCount(): number {
    return this.events.filter((event) => event.readAt === null).length;
  }

  /** Marks the named notifications read, or all of them. Returns how many changed. */
  markRead(ids: string[] | 'all'): number {
    const wanted = ids === 'all' ? null : new Set(ids);
    const readAt = new Date(this.now()).toISOString();
    const changed = this.events.filter((event) => event.readAt === null && (!wanted || wanted.has(event.id)));
    if (changed.length === 0) return 0;
    for (const event of changed) event.readAt = readAt;
    this.persist();
    for (const event of changed) this.emit({ kind: 'updated', event });
    this.emit({ kind: 'unread', count: this.unreadCount() });
    return changed.length;
  }

  private recordDeliveries(id: string, receipts: DeliveryReceipt[]): void {
    const event = this.events.find((candidate) => candidate.id === id);
    if (!event || receipts.length === 0) return;
    event.deliveries = [...(event.deliveries ?? []), ...receipts];
    this.persist();
    this.emit({ kind: 'updated', event });
  }

  private emit(change: ActivityFeedChange): void {
    for (const listener of this.listeners) listener(change);
  }

  /** Keeps the newest routine events up to the cap, plus every notified event
   * inside the retention window, so a flood of routine events cannot evict
   * the notification an operator is looking for. Returns whether it dropped any. */
  private trim(): boolean {
    const cutoff = this.now() - NOTIFIED_RETENTION_MS;
    const retained = (event: ActivityEvent) => notifiableActivity(event) && Date.parse(event.occurredAt) >= cutoff;
    const routine = this.events.filter((event) => !retained(event));
    if (routine.length <= MAX_STORED_EVENTS) return false;
    const evicted = new Set(routine.slice(0, routine.length - MAX_STORED_EVENTS));
    this.events = this.events.filter((event) => !evicted.has(event));
    return true;
  }

  private persist(): void {
    if (!this.path) return;
    mkdirSync(dirname(this.path), { recursive: true });
    const temporary = `${this.path}.${process.pid}.tmp`;
    writeFileSync(temporary, this.events.map((item) => JSON.stringify(item)).join('\n') + '\n', { mode: 0o600 });
    renameSync(temporary, this.path);
  }
}
