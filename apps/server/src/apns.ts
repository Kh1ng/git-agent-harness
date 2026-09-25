import crypto from 'node:crypto';
import { connect } from 'node:http2';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { ActivityEvent } from '@git-agent-harness/contracts';
import type { ChatLifecycleEvent } from './activityFeed.js';
import { activityPushPayload } from './webPush.js';
import { pushRegistrationId, validPushDeviceLabel, writePrivatePushStore } from './pushStore.js';

type ApnsConfig = {
  keyPath: string;
  keyId: string;
  teamId: string;
  bundleId: string;
  environment: 'production' | 'sandbox';
};
type ApnsResponse = { status: number; reason?: string };
export type ApnsRequest = { token: string; headers: Record<string, string>; payload: object };
type ApnsTransport = (host: string, request: ApnsRequest) => Promise<ApnsResponse>;
type LiveActivityRegistration = { profile: string; sessionId: string; token: string };
type TokenSlot = 'device' | 'pushToStartToken' | { liveActivity: string };
type StoredDevice = {
  id: string;
  token: string;
  label: string | null;
  pushToStartToken: string | null;
  liveActivities: Record<string, string>;
  createdAt: string;
  deviceId?: string;
};

const TOKEN_PATTERN = /^[a-fA-F0-9]{32,512}$/;
const UPDATE_INTERVAL_MS = 5_000;

function activityKey(profile: string, sessionId?: string): string {
  return `${profile}:${sessionId || 'default'}`;
}

function validToken(value: unknown): value is string {
  return typeof value === 'string' && TOKEN_PATTERN.test(value);
}

function defaultTransport(host: string, request: ApnsRequest): Promise<ApnsResponse> {
  return new Promise((resolveRequest, reject) => {
    const client = connect(host);
    const stream = client.request({ ':method': 'POST', ':path': `/3/device/${request.token}`, ...request.headers });
    const chunks: Buffer[] = [];
    let status = 0;
    let settled = false;
    const fail = (error: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.close();
      reject(error);
    };
    const timer = setTimeout(() => { stream.close(); fail(new Error('APNs request timed out')); }, 10_000);
    stream.on('response', (headers) => { status = Number(headers[':status'] ?? 0); });
    stream.on('data', (chunk) => chunks.push(Buffer.from(chunk)));
    stream.on('end', () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.close();
      let reason: string | undefined;
      try { reason = JSON.parse(Buffer.concat(chunks).toString('utf8')).reason; } catch { /* APNs success has no body. */ }
      resolveRequest({ status, reason });
    });
    stream.on('error', fail);
    client.on('error', fail);
    stream.end(JSON.stringify(request.payload));
  });
}

export class ApnsNotifications {
  private readonly key: crypto.KeyObject;
  private jwt: { value: string; issuedAt: number } | null = null;
  private lastActivityUpdate = new Map<string, number>();
  private activityStartedAt = new Map<string, number>();

  constructor(
    private readonly config: ApnsConfig,
    private readonly devicesPath = process.env.GAH_APNS_DEVICES_PATH ?? resolve(process.cwd(), 'config/push/apns-devices.json'),
    private readonly transport: ApnsTransport = defaultTransport,
    private readonly now: () => number = Date.now
  ) {
    this.key = crypto.createPrivateKey(readFileSync(config.keyPath));
  }

  list(): { count: number } {
    return { count: this.devices().length };
  }

  register(input: unknown, deviceId?: string): { id: string; count: number } {
    if (!input || typeof input !== 'object') throw new Error('An APNs device token is required.');
    const value = input as Record<string, unknown>;
    if (!validToken(value.token)) throw new Error('A valid APNs device token is required.');
    if (value.pushToStartToken !== undefined && value.pushToStartToken !== null && !validToken(value.pushToStartToken)) {
      throw new Error('Invalid Live Activity push-to-start token.');
    }
    if (!validPushDeviceLabel(value.label)) {
      throw new Error('Device label must be at most 80 printable characters.');
    }
    const live = value.liveActivity;
    let liveActivity: LiveActivityRegistration | null = null;
    if (live !== undefined) {
      if (!live || typeof live !== 'object') throw new Error('Invalid Live Activity registration.');
      const item = live as Record<string, unknown>;
      if (typeof item.profile !== 'string' || !item.profile || typeof item.sessionId !== 'string' || !validToken(item.token)) {
        throw new Error('Invalid Live Activity registration.');
      }
      liveActivity = item as LiveActivityRegistration;
    }
    const id = pushRegistrationId(value.token);
    const devices = this.devices();
    const previous = devices.find((device) => device.id === id);
    const device: StoredDevice = {
      id,
      token: value.token,
      label: typeof value.label === 'string' && value.label.trim() ? value.label.trim() : previous?.label ?? null,
      pushToStartToken: typeof value.pushToStartToken === 'string' ? value.pushToStartToken : previous?.pushToStartToken ?? null,
      liveActivities: { ...(previous?.liveActivities ?? {}) },
      createdAt: previous?.createdAt ?? new Date(this.now()).toISOString(),
      ...(deviceId ? { deviceId } : previous?.deviceId ? { deviceId: previous.deviceId } : {})
    };
    if (liveActivity) device.liveActivities[activityKey(liveActivity.profile, liveActivity.sessionId)] = liveActivity.token;
    writePrivatePushStore(this.devicesPath, [...devices.filter((entry) => entry.id !== id), device]);
    return { id, count: devices.some((entry) => entry.id === id) ? devices.length : devices.length + 1 };
  }

  remove(id: string): { removed: boolean; count: number } {
    if (!/^[a-f0-9]{24}$/.test(id)) throw new Error('Invalid APNs device id.');
    const before = this.devices();
    const after = before.filter((device) => device.id !== id);
    if (after.length !== before.length) writePrivatePushStore(this.devicesPath, after);
    return { removed: after.length !== before.length, count: after.length };
  }

  removeForDevice(deviceId: string): void {
    const devices = this.devices();
    const remaining = devices.filter((device) => device.deviceId !== deviceId);
    if (remaining.length !== devices.length) writePrivatePushStore(this.devicesPath, remaining);
  }

  async deliverActivity(event: ActivityEvent): Promise<void> {
    const payload = activityPushPayload(event);
    if (!payload) return;
    await this.sendToDevices((device) => ({
      token: device.token,
      headers: this.headers('alert', this.config.bundleId, event.id),
      payload: { aps: { alert: { title: payload.title, body: payload.body }, sound: 'default' }, ...payload }
    }), 'device');
  }

  async deliverChatLifecycle(event: ChatLifecycleEvent): Promise<void> {
    const key = activityKey(event.profile, event.sessionId);
    const state = event.phase === 'start' ? 'thinking'
      : event.phase === 'tool' ? `running ${event.tool ?? 'tool'}`
      : event.phase === 'permission' ? 'waiting for permission'
      : event.outcome === 'complete' ? 'done'
      : event.outcome === 'cancelled' ? 'cancelled' : 'failed';
    if (event.phase === 'start') this.activityStartedAt.set(key, Date.parse(event.occurredAt));
    const startedAt = this.activityStartedAt.get(key) ?? Date.parse(event.occurredAt);
    const content = { state, backend: event.backend ?? '', model: event.model ?? '', startedAt: Math.floor(startedAt / 1_000) };
    if (event.phase === 'start') {
      const payload = {
        aps: {
          timestamp: Math.floor(this.now() / 1_000), event: 'start', alert: { title: event.profile, body: 'Chat turn started' },
          'content-state': content, 'attributes-type': 'GAHLiveActivityAttributes',
          attributes: { project: event.profile, sessionId: event.sessionId ?? 'default' }, 'input-push-token': 1
        }
      };
      await this.sendToDevices((device) => device.pushToStartToken ? {
        token: device.pushToStartToken,
        headers: this.headers('liveactivity', `${this.config.bundleId}.push-type.liveactivity`),
        payload
      } : null, 'pushToStartToken');
      return;
    }
    const end = event.phase === 'end';
    const now = this.now();
    if (!end && event.phase !== 'permission' && now - (this.lastActivityUpdate.get(key) ?? 0) < UPDATE_INTERVAL_MS) return;
    if (!end) this.lastActivityUpdate.set(key, now); else {
      this.lastActivityUpdate.delete(key);
      this.activityStartedAt.delete(key);
    }
    const aps: Record<string, unknown> = {
      timestamp: Math.floor(now / 1_000), event: end ? 'end' : 'update', 'content-state': content
    };
    if (end) aps['dismissal-date'] = Math.floor((now + (event.outcome === 'cancelled' ? 0 : 15 * 60_000)) / 1_000);
    await this.sendToDevices((device) => {
      const token = device.liveActivities[key];
      return token ? { token, headers: this.headers('liveactivity', `${this.config.bundleId}.push-type.liveactivity`), payload: { aps } } : null;
    }, { liveActivity: key });
  }

  private headers(pushType: 'alert' | 'liveactivity', topic: string, collapseId?: string): Record<string, string> {
    return {
      authorization: `bearer ${this.authorization()}`,
      'apns-topic': topic,
      'apns-push-type': pushType,
      'apns-priority': '10',
      ...(collapseId ? { 'apns-collapse-id': collapseId.slice(0, 64) } : {})
    };
  }

  private authorization(): string {
    const issuedAt = Math.floor(this.now() / 1_000);
    if (this.jwt && issuedAt - this.jwt.issuedAt < 50 * 60) return this.jwt.value;
    const encode = (value: unknown) => Buffer.from(JSON.stringify(value)).toString('base64url');
    const unsigned = `${encode({ alg: 'ES256', kid: this.config.keyId })}.${encode({ iss: this.config.teamId, iat: issuedAt })}`;
    const signature = crypto.sign('sha256', Buffer.from(unsigned), { key: this.key, dsaEncoding: 'ieee-p1363' }).toString('base64url');
    this.jwt = { value: `${unsigned}.${signature}`, issuedAt };
    return this.jwt.value;
  }

  private async sendToDevices(
    request: (device: StoredDevice) => ApnsRequest | null,
    tokenSlot: TokenSlot
  ): Promise<void> {
    const host = this.config.environment === 'production' ? 'https://api.push.apple.com' : 'https://api.sandbox.push.apple.com';
    const devices = this.devices();
    const invalid = new Map<string, Map<string, string>>();
    await Promise.all(devices.map(async (device) => {
      try {
        const outgoing = request(device);
        if (!outgoing) return;
        if (Buffer.byteLength(JSON.stringify(outgoing.payload)) > 4_096) {
          console.error(`[apns] skipped oversized payload for device ${device.id}`);
          return;
        }
        const response = await this.transport(host, outgoing);
        if (response.status === 410 || response.reason === 'BadDeviceToken' || response.reason === 'Unregistered') {
          const kinds = invalid.get(device.id) ?? new Map<string, string>();
          kinds.set(typeof tokenSlot === 'string' ? tokenSlot : tokenSlot.liveActivity, outgoing.token);
          invalid.set(device.id, kinds);
        } else if (response.status >= 400) {
          console.error(`[apns] delivery failed for device ${device.id}: ${response.reason ?? response.status}`);
        }
      } catch (error) {
        console.error(`[apns] delivery failed for device ${device.id}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }));
    if (!invalid.size) return;
    writePrivatePushStore(this.devicesPath, this.devices().flatMap((device) => {
      const kinds = invalid.get(device.id);
      if (!kinds) return [device];
      if (kinds.get('device') === device.token) return [];
      const liveActivities = { ...device.liveActivities };
      for (const [kind, failedToken] of kinds) {
        if (kind !== 'device' && kind !== 'pushToStartToken' && liveActivities[kind] === failedToken) delete liveActivities[kind];
      }
      return [{
        ...device,
        pushToStartToken: kinds.get('pushToStartToken') === device.pushToStartToken ? null : device.pushToStartToken,
        liveActivities
      }];
    }));
  }

  private devices(): StoredDevice[] {
    if (!existsSync(this.devicesPath)) return [];
    const devices = JSON.parse(readFileSync(this.devicesPath, 'utf8')) as StoredDevice[];
    if (!Array.isArray(devices)) throw new Error(`Invalid APNs device store at ${this.devicesPath}`);
    return devices.filter((device) => validToken(device?.token) && typeof device.id === 'string');
  }
}

export function apnsFromEnvironment(): ApnsNotifications | undefined {
  const keyPath = process.env.GAH_APNS_KEY_PATH;
  const keyId = process.env.GAH_APNS_KEY_ID;
  const teamId = process.env.GAH_APNS_TEAM_ID;
  if (!keyPath || !keyId || !teamId) {
    console.log('APNs disabled: set GAH_APNS_KEY_PATH, GAH_APNS_KEY_ID, and GAH_APNS_TEAM_ID to enable it.');
    return undefined;
  }
  return new ApnsNotifications({
    keyPath,
    keyId,
    teamId,
    bundleId: process.env.GAH_APNS_BUNDLE_ID ?? 'com.kh1ng.gah.controller',
    environment: process.env.GAH_APNS_ENVIRONMENT === 'production' ? 'production' : 'sandbox'
  });
}
