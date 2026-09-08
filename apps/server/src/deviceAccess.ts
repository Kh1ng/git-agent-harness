import crypto from 'node:crypto';
import { closeSync, mkdirSync, openSync, readSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import type { PairedDevice, PairingOffer, PairingPreview } from '@git-agent-harness/contracts';

export const DEVICE_COOKIE = 'gah_device';
export const DEVICE_ACCESS = 'Dashboard control: read projects and chats, run agent work, and choose chat models. The owner controls pairing, credentials, worker registration, global settings, and destructive administration.';
const CODE_LIFETIME = 5 * 60_000;
export const DEVICE_LIFETIME = 30 * 24 * 60 * 60_000;
type StoredDevice = PairedDevice & { token_hash: string };
const hash = (value: string) => crypto.createHash('sha256').update(value).digest('hex');
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** One central process owns pending offers. Only hashes and device metadata persist.
 * Synchronous redemption/persistence leaves no async gap for a second redemption. */
export class DeviceAccess {
  private offers = new Map<string, PairingOffer>();
  private devices: StoredDevice[] | undefined;
  private revokedListeners = new Set<(id: string) => void>();
  constructor(private path = process.env.GAH_DEVICE_STORE_PATH ?? resolve('config/paired-devices.json'), private now = Date.now) {}

  private records(): StoredDevice[] {
    if (this.devices) return this.devices;
    let raw: string;
    let file: number | undefined;
    try {
      file = openSync(this.path, 'r');
      const bytes = Buffer.alloc(1_000_001);
      let length = 0;
      while (length < bytes.length) {
        const count = readSync(file, bytes, length, bytes.length - length, null);
        if (count === 0) break;
        length += count;
      }
      if (length > 1_000_000) throw new Error();
      raw = bytes.subarray(0, length).toString('utf8');
    }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return this.devices = [];
      throw new Error('Cannot read paired device storage.');
    } finally { if (file !== undefined) closeSync(file); }
    try {
      const stored = JSON.parse(raw);
      if (stored.schema_version !== 1 || !Array.isArray(stored.devices) || stored.devices.length > 1000) throw new Error();
      const ids = new Set<string>();
      for (const d of stored.devices) {
        if (typeof d.id !== 'string' || !uuid.test(d.id) || ids.has(d.id) || typeof d.name !== 'string' || !d.name.trim() || d.name.length > 80
          || typeof d.token_hash !== 'string' || !/^[0-9a-f]{64}$/.test(d.token_hash)
          || typeof d.created_at !== 'string' || !Number.isFinite(Date.parse(d.created_at))
          || typeof d.expires_at !== 'string' || !Number.isFinite(Date.parse(d.expires_at))
          || (d.revoked_at !== null && (typeof d.revoked_at !== 'string' || !Number.isFinite(Date.parse(d.revoked_at))))) throw new Error();
        ids.add(d.id);
      }
      return this.devices = stored.devices;
    } catch { throw new Error('Paired device storage is invalid; access is denied.'); }
  }

  private persist(devices: StoredDevice[]): void {
    const temporary = `${this.path}.${crypto.randomUUID()}.tmp`;
    try {
      mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
      writeFileSync(temporary, JSON.stringify({ schema_version: 1, devices }), { mode: 0o600, flag: 'wx' });
      renameSync(temporary, this.path);
      this.devices = devices;
    } catch {
      try { rmSync(temporary, { force: true }); } catch { /* Keep the original, redacted storage error. */ }
      throw new Error('Cannot save paired device storage.');
    }
  }

  create(server: PairingOffer['server']): PairingOffer {
    for (const [code, offer] of this.offers) if (Date.parse(offer.expires_at) <= this.now()) this.offers.delete(code);
    if (this.offers.size >= 20) throw new Error('Too many pending pairing codes. Wait for an existing code to expire.');
    const code = crypto.randomBytes(24).toString('base64url');
    const offer: PairingOffer = { schema_version: 1, code, server, access: DEVICE_ACCESS, expires_at: new Date(this.now() + CODE_LIFETIME).toISOString() };
    this.offers.set(code, offer);
    return offer;
  }

  inspect(code: unknown, serverId: unknown, origin: string): PairingPreview {
    const offer = typeof code === 'string' && /^[A-Za-z0-9_-]{32}$/.test(code) ? this.offers.get(code) : undefined;
    if (!offer || Date.parse(offer.expires_at) <= this.now() || offer.server.id !== serverId || offer.server.origin !== origin) {
      throw new Error('Pairing code is expired, already used, or belongs to another server. Generate a new code.');
    }
    const { code: _code, ...preview } = offer;
    return preview;
  }

  redeem(code: unknown, serverId: unknown, origin: string, name: unknown): { device: PairedDevice; token: string } {
    this.inspect(code, serverId, origin);
    if (typeof name !== 'string' || !name.trim() || name.length > 80 || /[\x00-\x1f\x7f]/.test(name)) throw new Error('Enter a device name of 1–80 characters.');
    const records = this.records();
    if (records.length >= 1000) throw new Error('Paired device storage is full. Ask the owner to maintain the device store.');
    const token = crypto.randomBytes(32).toString('base64url');
    const device: PairedDevice = { id: crypto.randomUUID(), name: name.trim(), created_at: new Date(this.now()).toISOString(), expires_at: new Date(this.now() + DEVICE_LIFETIME).toISOString(), revoked_at: null };
    this.persist([...records, { ...device, token_hash: hash(token) }]);
    this.offers.delete(code as string);
    return { device, token: `${device.id}.${token}` };
  }

  authenticate(token: string): PairedDevice | null {
    const [id, secret, extra] = token.split('.');
    if (extra !== undefined || !uuid.test(id ?? '') || !/^[A-Za-z0-9_-]{43}$/.test(secret ?? '')) return null;
    const record = this.records().find(device => device.id === id);
    if (!record || !this.active(id) || !crypto.timingSafeEqual(Buffer.from(record.token_hash, 'hex'), Buffer.from(hash(secret), 'hex'))) return null;
    const { token_hash: _hash, ...device } = record;
    return device;
  }

  active(id: string): boolean {
    return this.records().some(device => device.id === id && device.revoked_at === null && Date.parse(device.expires_at) > this.now());
  }

  list(): PairedDevice[] {
    return this.records().map(({ token_hash: _hash, ...device }) => device);
  }

  revoke(id: string): void {
    if (!this.records().some(device => device.id === id)) throw new Error('Paired device not found.');
    this.persist(this.records().map(device => device.id === id && device.revoked_at === null ? { ...device, revoked_at: new Date(this.now()).toISOString() } : device));
    for (const listener of this.revokedListeners) listener(id);
  }

  onRevoke(listener: (id: string) => void): () => void {
    this.revokedListeners.add(listener);
    return () => { this.revokedListeners.delete(listener); };
  }
}

/** Reject duplicate cookie credentials rather than guessing which authority wins. */
export function deviceCookie(header: string | undefined): string | undefined {
  const values = (header ?? '').split(';').map(part => part.trim()).filter(part => part.startsWith(`${DEVICE_COOKIE}=`));
  return values.length === 0 ? undefined : values.length === 1 ? values[0].slice(DEVICE_COOKIE.length + 1) : '';
}
