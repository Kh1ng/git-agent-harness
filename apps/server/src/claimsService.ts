/**
 * Central claim arbitration (issue #882): the shared exclusivity gate that
 * lets `gah dispatch`/`gah loop` on different nodes safely avoid picking up
 * the same work_id twice. Replaces (only when `registry_central_url` is
 * configured -- see src/central_claims.rs on the Rust side) the local
 * ledger-claim mechanism's cross-node role; the *local* PID+file lock
 * (work_claim.rs) is untouched, it protects a different, single-machine
 * problem.
 *
 * Storage is a plain JSON file + in-memory Map, matching registryService.ts's
 * own load()/save() pattern -- not SQLite. Correctness doesn't need a real
 * database here: as long as acquire/renew/release are synchronous functions
 * (no `await` between the read-check and the write), Node's single-threaded
 * event loop already guarantees no other request can interleave in the
 * middle, which is the same atomicity a `WHERE` clause buys you, without a
 * new native dependency for a three-column table.
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import type { ClaimLease } from '@git-agent-harness/contracts';

const DEFAULT_LEASE_SECONDS = 15 * 60; // 15 min; renewal interval is lease/3, see central_claims.rs
const MIN_LEASE_SECONDS = 60;
const MAX_LEASE_SECONDS = 60 * 60;

export class ClaimConflictError extends Error {
  constructor(public readonly heldBy: ClaimLease) {
    super(`work_id '${heldBy.work_id}' on profile '${heldBy.profile}' is already claimed by node '${heldBy.node_id}'`);
  }
}

function claimKey(profile: string, workId: string): string {
  return `${profile}::${workId}`;
}

function clampLeaseSeconds(requested: number | undefined): number {
  const value = requested ?? DEFAULT_LEASE_SECONDS;
  return Math.min(MAX_LEASE_SECONDS, Math.max(MIN_LEASE_SECONDS, value));
}

export class ClaimsService {
  private configPath: string;
  private leases: Map<string, ClaimLease> = new Map();

  constructor(configPath?: string) {
    this.configPath = configPath || process.env.GAH_CLAIMS_CONFIG_PATH || resolve(process.cwd(), 'config/claims-config.json');
    this.load();
  }

  private load() {
    if (existsSync(this.configPath)) {
      try {
        const data = JSON.parse(readFileSync(this.configPath, 'utf8'));
        if (Array.isArray(data.leases)) {
          for (const lease of data.leases as ClaimLease[]) {
            this.leases.set(claimKey(lease.profile, lease.work_id), lease);
          }
        }
      } catch (e) {
        console.error('Failed to load claims config:', e);
      }
    }
  }

  private save() {
    try {
      const dir = dirname(this.configPath);
      if (!existsSync(dir)) {
        mkdirSync(dir, { recursive: true });
      }
      writeFileSync(this.configPath, JSON.stringify({ leases: Array.from(this.leases.values()) }, null, 2));
    } catch (e) {
      console.error('Failed to save claims config:', e);
      throw e;
    }
  }

  /** Deletes any lease past its expiry -- opportunistic GC piggybacked on
   * real traffic instead of a separate sweep timer. Not called for its own
   * sake; every mutating call below runs it first so reads never have to
   * special-case a stale entry. */
  private sweepExpired(nowMs: number): void {
    for (const [key, lease] of this.leases) {
      if (Date.parse(lease.expires_at) <= nowMs) {
        this.leases.delete(key);
      }
    }
  }

  /** Synchronous by design -- see module docs. Throws ClaimConflictError
   * (caller maps to HTTP 409) when a different node currently holds an
   * unexpired lease. Re-acquiring your own already-held claim is treated as
   * idempotent (refreshes the lease) rather than a conflict, since a worker
   * retrying its own acquire call after a network blip is the common case,
   * not an error. */
  acquire(nodeId: string, profile: string, workId: string, leaseSeconds?: number): ClaimLease {
    const now = Date.now();
    this.sweepExpired(now);
    const key = claimKey(profile, workId);
    const existing = this.leases.get(key);
    if (existing && existing.node_id !== nodeId) {
      throw new ClaimConflictError(existing);
    }
    const nowIso = new Date(now).toISOString();
    const lease: ClaimLease = {
      profile,
      work_id: workId,
      node_id: nodeId,
      claimed_at: existing?.claimed_at ?? nowIso,
      renewed_at: nowIso,
      expires_at: new Date(now + clampLeaseSeconds(leaseSeconds) * 1000).toISOString()
    };
    this.leases.set(key, lease);
    this.save();
    return lease;
  }

  /** Throws ClaimConflictError if the lease doesn't exist, expired, or is
   * held by a different node -- a renewal can't resurrect a lease that
   * already lapsed and may have been picked up by someone else. */
  renew(nodeId: string, profile: string, workId: string, leaseSeconds?: number): ClaimLease {
    const now = Date.now();
    this.sweepExpired(now);
    const key = claimKey(profile, workId);
    const existing = this.leases.get(key);
    if (!existing || existing.node_id !== nodeId) {
      throw new ClaimConflictError(
        existing ?? { profile, work_id: workId, node_id: '(none)', claimed_at: '', renewed_at: '', expires_at: '' }
      );
    }
    const nowIso = new Date(now).toISOString();
    const lease: ClaimLease = {
      ...existing,
      renewed_at: nowIso,
      expires_at: new Date(now + clampLeaseSeconds(leaseSeconds) * 1000).toISOString()
    };
    this.leases.set(key, lease);
    this.save();
    return lease;
  }

  /** No-op (not an error) if the lease is already gone or held by someone
   * else -- release is best-effort cleanup, not a claim you can fight over. */
  release(nodeId: string, profile: string, workId: string): void {
    const key = claimKey(profile, workId);
    const existing = this.leases.get(key);
    if (existing && existing.node_id === nodeId) {
      this.leases.delete(key);
      this.save();
    }
  }

  getLease(profile: string, workId: string): ClaimLease | undefined {
    this.sweepExpired(Date.now());
    return this.leases.get(claimKey(profile, workId));
  }
}
