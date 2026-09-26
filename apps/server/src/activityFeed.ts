import crypto from 'node:crypto';
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import type { ActivityEvent, ControllerEvent, GatewayHealthSummary, QuotaSnapshot } from '@git-agent-harness/contracts';
import { controllerDispatchSucceeded } from './controllerActivity.js';
import { redactTextSecrets } from './managerChat/redactText.js';

const MAX_STORED_EVENTS = 2_000;
const REPLAY_LIMIT = 200;

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

function permissionTool(value?: string): string {
  const name = value?.match(/^[\p{L}\p{N}_-]+/u)?.[0];
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

export class ActivityFeed {
  private events: ActivityEvent[] = [];
  private ids = new Set<string>();

  constructor(
    private path: string | null = process.env.GAH_ACTIVITY_PATH ?? resolve(process.cwd(), 'config/activity.jsonl'),
    private onRecorded?: (event: ActivityEvent) => void | Promise<void>
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

  record(event: ActivityEvent, deliver = true): boolean {
    if (this.ids.has(event.id)) return false;
    this.events.push(event);
    this.ids.add(event.id);
    if (this.path) {
      mkdirSync(dirname(this.path), { recursive: true });
      appendFileSync(this.path, `${JSON.stringify(event)}\n`, { mode: 0o600 });
    }
    if (this.events.length > MAX_STORED_EVENTS) {
      this.trim();
      if (this.path) writeFileSync(this.path, this.events.map((item) => JSON.stringify(item)).join('\n') + '\n', { mode: 0o600 });
    }
    if (deliver && this.onRecorded) {
      void Promise.resolve().then(() => this.onRecorded?.(event)).catch((error) => {
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

  private trim(): void {
    this.events = this.events.slice(-MAX_STORED_EVENTS);
  }
}
