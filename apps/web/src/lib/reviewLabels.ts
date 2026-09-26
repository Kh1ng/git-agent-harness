import type { MergeRequest } from '@git-agent-harness/contracts';

/** Shared labels for the PR/MR summary surfaces (review panel, work drawer). */
export function ciLabelFor(mergeRequest?: MergeRequest | null): string {
  return mergeRequest?.ci_pending ? 'Pending' : mergeRequest?.ci_passed ? 'Passed' : mergeRequest ? 'Not passing' : 'Unknown';
}

export function stateLabelFor(mergeRequest?: MergeRequest | null, providerOpen = false, existing = false): string {
  const state = mergeRequest?.state ?? (providerOpen || existing ? 'open' : 'Not published');
  return state.toLowerCase().replaceAll('_', ' ');
}
