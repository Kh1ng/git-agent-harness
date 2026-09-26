import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, mkdirSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';

export function pushRegistrationId(value: string): string {
  return createHash('sha256').update(value).digest('hex').slice(0, 24);
}

export function validPushRegistrationId(value: unknown): value is string {
  return typeof value === 'string' && /^[a-f0-9]{24}$/.test(value);
}

export type StoredPushEntry = { id: string; deviceId?: string };

/** Remove entries matching a predicate and persist the remainder with the same private-write rules. */
export function removePushEntries<T extends StoredPushEntry>(
  path: string,
  entries: T[],
  matches: (entry: T) => boolean
): { removed: boolean; remaining: T[] } {
  const remaining = entries.filter((entry) => !matches(entry));
  if (remaining.length !== entries.length) writePrivatePushStore(path, remaining);
  return { removed: remaining.length !== entries.length, remaining };
}

export function validPushDeviceLabel(value: unknown): boolean {
  return value === undefined
    || (typeof value === 'string' && value.length <= 80 && !/[\x00-\x1f\x7f]/.test(value));
}

/** Atomically replace a push store without making its contents group-readable. */
export function writePrivatePushStore(path: string, value: unknown): void {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.tmp-${process.pid}-${randomUUID()}`;
  try {
    writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
    renameSync(temporary, path);
    chmodSync(path, 0o600);
  } catch (error) {
    try { unlinkSync(temporary); } catch { /* preserve the primary error */ }
    throw error;
  }
}
