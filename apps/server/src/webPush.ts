import {
  chmodSync,
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  unlinkSync,
  writeFileSync
} from 'node:fs';
import { dirname, resolve } from 'node:path';
import { activityPath, notifiableActivity, type ActivityEvent } from '@git-agent-harness/contracts';
import webPush, { type PushSubscription } from 'web-push';
import { pushRegistrationId, removePushEntries, validPushDeviceLabel, validPushRegistrationId, writePrivatePushStore } from './pushStore.js';

const MAX_PAYLOAD_BYTES = 3 * 1024;
const FAILURE_LOG_INTERVAL_MS = 60 * 60 * 1_000;
const PUSH_ENDPOINT_HOSTS = new Set(['fcm.googleapis.com', 'updates.push.services.mozilla.com']);
const PUSH_ENDPOINT_SUFFIXES = ['.push.apple.com', '.notify.windows.com'];

type VapidKeys = { publicKey: string; privateKey: string };
type StoredSubscription = {
  id: string;
  label: string | null;
  createdAt: string;
  subscription: PushSubscription;
  deviceId?: string;
};
type WebPushTransport = Pick<typeof webPush, 'generateVAPIDKeys' | 'setVapidDetails' | 'sendNotification'>;

export type ActivityPushPayload = { id: string; title: string; body: string; url: string };

/** The shared wake filter and bounded public payload for every push transport. */
export function activityPushPayload(event: ActivityEvent): ActivityPushPayload | null {
  if (!notifiableActivity(event)) return null;
  const payload = {
    id: event.id,
    title: event.title.slice(0, 120),
    body: event.message.slice(0, 500),
    url: activityPath(event)
  };
  return Buffer.byteLength(JSON.stringify(payload)) <= MAX_PAYLOAD_BYTES ? payload : null;
}

function readJson<T>(path: string, fallback: T): T {
  if (!existsSync(path)) return fallback;
  return JSON.parse(readFileSync(path, 'utf8')) as T;
}

function validSubscription(value: unknown): value is PushSubscription {
  if (!value || typeof value !== 'object') return false;
  const input = value as Record<string, unknown>;
  if (typeof input.endpoint !== 'string') return false;
  try {
    const endpoint = new URL(input.endpoint);
    if (endpoint.protocol !== 'https:'
      || (!PUSH_ENDPOINT_HOSTS.has(endpoint.hostname)
        && !PUSH_ENDPOINT_SUFFIXES.some((suffix) => endpoint.hostname.endsWith(suffix)))) return false;
  } catch {
    return false;
  }
  const keys = input.keys;
  return !!keys && typeof keys === 'object'
    && typeof (keys as Record<string, unknown>).p256dh === 'string'
    && typeof (keys as Record<string, unknown>).auth === 'string';
}

export class WebPushNotifications {
  private readonly keys: VapidKeys;
  private failures = new Map<string, number>();

  constructor(
    private readonly keysPath = process.env.GAH_VAPID_KEYS_PATH ?? resolve(process.cwd(), 'config/push/vapid.json'),
    private readonly subscriptionsPath = process.env.GAH_WEB_PUSH_SUBSCRIPTIONS_PATH ?? resolve(process.cwd(), 'config/push/subscriptions.json'),
    private readonly transport: WebPushTransport = webPush
  ) {
    this.keys = this.loadOrCreateKeys();
    this.transport.setVapidDetails(
      process.env.GAH_VAPID_SUBJECT ?? 'mailto:operator@localhost',
      this.keys.publicKey,
      this.keys.privateKey
    );
  }

  publicKey(): string {
    return this.keys.publicKey;
  }

  list(): { count: number } {
    return { count: this.subscriptions().length };
  }

  register(value: unknown, label?: unknown, deviceId?: string): { id: string; count: number } {
    if (!validSubscription(value)) throw new Error('A valid HTTPS push subscription is required.');
    if (!validPushDeviceLabel(label)) {
      throw new Error('Device label must be at most 80 printable characters.');
    }
    const id = pushRegistrationId(value.endpoint);
    const stored = this.subscriptions();
    const existing = stored.find((entry) => entry.id === id);
    const subscriptions = stored.filter((entry) => entry.id !== id);
    subscriptions.push({
      id,
      label: typeof label === 'string' && label.trim() ? label.trim() : null,
      createdAt: new Date().toISOString(),
      subscription: value,
      ...(deviceId ? { deviceId } : existing?.deviceId ? { deviceId: existing.deviceId } : {})
    });
    writePrivatePushStore(this.subscriptionsPath, subscriptions);
    return { id, count: subscriptions.length };
  }

  remove(id: string): { removed: boolean; count: number } {
    if (!validPushRegistrationId(id)) throw new Error('Invalid push subscription id.');
    const { removed, remaining } = removePushEntries(this.subscriptionsPath, this.subscriptions(), (entry) => entry.id === id);
    return { removed, count: remaining.length };
  }

  removeForDevice(deviceId: string): void {
    removePushEntries(this.subscriptionsPath, this.subscriptions(), (entry) => entry.deviceId === deviceId);
  }

  async deliverActivity(event: ActivityEvent): Promise<void> {
    const publicPayload = activityPushPayload(event);
    if (!publicPayload) {
      if (notifiableActivity(event)) console.error(`[webPush] skipped oversized activity payload ${event.id}`);
      return;
    }
    const payload = JSON.stringify(publicPayload);
    const subscriptions = this.subscriptions();
    const expired = new Set<string>();
    await Promise.all(subscriptions.map(async (entry) => {
      try {
        await this.transport.sendNotification(entry.subscription, payload, { TTL: 60 * 60, timeout: 10_000 });
        this.failures.delete(entry.id);
      } catch (error) {
        const status = (error as { statusCode?: number }).statusCode;
        if (status === 404 || status === 410) {
          expired.add(entry.id);
          return;
        }
        const now = Date.now();
        if (now - (this.failures.get(entry.id) ?? 0) >= FAILURE_LOG_INTERVAL_MS) {
          this.failures.set(entry.id, now);
          console.error(`[webPush] delivery failed for subscription ${entry.id}: ${error instanceof Error ? error.message : String(error)}`);
        }
      }
    }));
    if (expired.size) writePrivatePushStore(this.subscriptionsPath, this.subscriptions().filter((entry) => !expired.has(entry.id)));
  }

  private loadOrCreateKeys(): VapidKeys {
    if (existsSync(this.keysPath)) return this.readKeys();
    mkdirSync(dirname(this.keysPath), { recursive: true, mode: 0o700 });
    const keys = this.transport.generateVAPIDKeys();
    let file: number;
    try {
      file = openSync(this.keysPath, 'wx', 0o600);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'EEXIST') return this.readKeys();
      throw error;
    }
    try {
      writeFileSync(file, `${JSON.stringify(keys, null, 2)}\n`);
      fsyncSync(file);
    } catch (error) {
      try { unlinkSync(this.keysPath); } catch { /* preserve the primary error */ }
      throw error;
    } finally {
      closeSync(file);
    }
    return keys;
  }

  private readKeys(): VapidKeys {
    const keys = readJson<Partial<VapidKeys>>(this.keysPath, {});
    if (!keys.publicKey || !keys.privateKey) throw new Error(`Invalid VAPID key file at ${this.keysPath}`);
    chmodSync(this.keysPath, 0o600);
    return { publicKey: keys.publicKey, privateKey: keys.privateKey };
  }

  private subscriptions(): StoredSubscription[] {
    const subscriptions = readJson<StoredSubscription[]>(this.subscriptionsPath, []);
    return Array.isArray(subscriptions)
      ? subscriptions.filter((entry) => typeof entry?.id === 'string' && validSubscription(entry.subscription))
      : [];
  }
}
