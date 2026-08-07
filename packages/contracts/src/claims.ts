// Issue #882: central claim arbitration. Distinct from ActiveClaim
// (gah.ts) -- that's a snapshot of a node's *local* work_claim.rs
// PID+file locks, surfaced for the advisory fleet preflight (#881). A
// ClaimLease is the actual cross-node exclusivity grant, held on the
// central node.

export interface ClaimAcquireRequest {
  node_id: string;
  profile: string;
  work_id: string;
  /** Seconds until the lease expires without a renewal. Server clamps to a
   * sane range; omit to use the server default. */
  lease_seconds?: number;
}

export interface ClaimLease {
  profile: string;
  work_id: string;
  node_id: string;
  claimed_at: string;
  renewed_at: string;
  expires_at: string;
}

export interface ClaimConflictResponse {
  error: 'Conflict';
  message: string;
  held_by: ClaimLease;
}

export interface ClaimRenewRequest {
  node_id: string;
  profile: string;
  work_id: string;
  lease_seconds?: number;
}

export interface ClaimReleaseRequest {
  node_id: string;
  profile: string;
  work_id: string;
}
