/**
 * Single cache for the pull-data REST endpoints (gahApi), so Overview,
 * Telemetry, Quota, and Work don't each independently re-fetch /api/status
 * or /api/report -- see Phase "Performance": do not repeatedly fetch the
 * same report from many components.
 *
 * zustand, not a new query-library dependency: it's already a dependency
 * of apps/web (unused until now) and this doesn't need react-query's
 * request deduping/retry machinery -- just "fetch once, share the result,
 * let the caller refresh."
 */
import { create } from 'zustand';
import { gahApi, GahApiError } from '../api/client.js';
import type { StatusSnapshot, ReportData, ReportGroupBy, LedgerEntry, ControllerEvent } from '@git-agent-harness/contracts';

interface Resource<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
  fetchedAt: number | null;
}

function emptyResource<T>(): Resource<T> {
  return { data: null, loading: false, error: null, fetchedAt: null };
}

interface GahStoreState {
  status: Resource<StatusSnapshot>;
  report: Resource<ReportData>;
  events: Resource<ControllerEvent[]>;
  workTimelines: Record<string, Resource<LedgerEntry[]>>;

  fetchStatus: (profile?: string, opts?: { force?: boolean }) => Promise<void>;
  fetchReport: (params?: { profile?: string; since?: string; groupBy?: ReportGroupBy }, opts?: { force?: boolean }) => Promise<void>;
  fetchEvents: (params?: { profile?: string; since?: string }, opts?: { force?: boolean }) => Promise<void>;
  fetchWorkTimeline: (workId: string, opts?: { force?: boolean }) => Promise<void>;
}

/** Below this age, a fetch* call reuses the cached value instead of
 * re-requesting -- short enough that "what's running now" stays honest,
 * long enough that switching between Overview/Telemetry/Quota in the same
 * minute doesn't re-hit the CLI three times. */
const FRESH_MS = 15_000;

function isFresh(resource: Resource<unknown>): boolean {
  return resource.fetchedAt !== null && Date.now() - resource.fetchedAt < FRESH_MS;
}

function errorMessage(error: unknown): string {
  if (error instanceof GahApiError) return error.message;
  return error instanceof Error ? error.message : String(error);
}

export const useGahStore = create<GahStoreState>((set, get) => ({
  status: emptyResource(),
  report: emptyResource(),
  events: emptyResource(),
  workTimelines: {},

  async fetchStatus(profile, opts) {
    const current = get().status;
    if (current.loading || (!opts?.force && isFresh(current))) return;
    set({ status: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getStatus(profile);
      set({ status: { data, loading: false, error: null, fetchedAt: Date.now() } });
    } catch (error) {
      set({ status: { ...get().status, loading: false, error: errorMessage(error) } });
    }
  },

  async fetchReport(params, opts) {
    const current = get().report;
    if (current.loading || (!opts?.force && isFresh(current))) return;
    set({ report: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getReport(params);
      set({ report: { data, loading: false, error: null, fetchedAt: Date.now() } });
    } catch (error) {
      set({ report: { ...get().report, loading: false, error: errorMessage(error) } });
    }
  },

  async fetchEvents(params, opts) {
    const current = get().events;
    if (current.loading || (!opts?.force && isFresh(current))) return;
    set({ events: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getEvents(params);
      set({ events: { data, loading: false, error: null, fetchedAt: Date.now() } });
    } catch (error) {
      set({ events: { ...get().events, loading: false, error: errorMessage(error) } });
    }
  },

  async fetchWorkTimeline(workId, opts) {
    const current = get().workTimelines[workId] ?? emptyResource<LedgerEntry[]>();
    if (current.loading || (!opts?.force && isFresh(current))) return;
    set({ workTimelines: { ...get().workTimelines, [workId]: { ...current, loading: true, error: null } } });
    try {
      const data = await gahApi.getWorkTimeline(workId);
      set({
        workTimelines: {
          ...get().workTimelines,
          [workId]: { data, loading: false, error: null, fetchedAt: Date.now() }
        }
      });
    } catch (error) {
      set({
        workTimelines: {
          ...get().workTimelines,
          [workId]: { ...current, loading: false, error: errorMessage(error) }
        }
      });
    }
  }
}));
