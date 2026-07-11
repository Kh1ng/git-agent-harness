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
import type { StatusSnapshot, ReportData, ReportSeriesData, ReportGroupBy, LedgerEntry, ControllerEvent, ProfileSummary } from '@git-agent-harness/contracts';
import type { ProfileAddData, ProfileUpdateData, ProfileRemoveParams, LoopStatus } from '../api/client.js';

interface Resource<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
  fetchedAt: number | null;
  /** Serialized fetch params (e.g. the profile) the current data was
   * fetched for. Switching profiles must always refetch even inside the
   * freshness window below -- otherwise the dashboard silently shows the
   * previous profile's numbers for up to FRESH_MS after a switch. */
  key: string | null;
}

function emptyResource<T>(): Resource<T> {
  return { data: null, loading: false, error: null, fetchedAt: null, key: null };
}

interface ProfileCrudState {
  adding: boolean;
  updating: boolean;
  removing: boolean;
  addError: string | null;
  updateError: string | null;
  removeError: string | null;
  lastAddSuccess: boolean;
  lastUpdateSuccess: boolean;
  lastRemoveSuccess: boolean;
}

interface GahStoreState {
  status: Resource<StatusSnapshot>;
  report: Resource<ReportData>;
  reportSeries: Resource<ReportSeriesData>;
  events: Resource<ControllerEvent[]>;
  workTimelines: Record<string, Resource<LedgerEntry[]>>;
  profiles: Resource<ProfileSummary[]>;
  profileCrud: ProfileCrudState;
  loopStatus: Resource<LoopStatus>;
  loopAction: { pending: boolean; error: string | null };

  fetchStatus: (profile?: string, opts?: { force?: boolean }) => Promise<void>;
  fetchReport: (params?: { profile?: string; since?: string; groupBy?: ReportGroupBy }, opts?: { force?: boolean }) => Promise<void>;
  fetchReportSeries: (params?: { profile?: string; since?: string; bucket?: string }, opts?: { force?: boolean }) => Promise<void>;
  fetchEvents: (params?: { profile?: string; since?: string }, opts?: { force?: boolean }) => Promise<void>;
  fetchWorkTimeline: (workId: string, opts?: { force?: boolean }) => Promise<void>;
  fetchProfiles: (opts?: { force?: boolean }) => Promise<void>;
  addProfile: (data: ProfileAddData) => Promise<void>;
  updateProfile: (name: string, data: ProfileUpdateData) => Promise<void>;
  removeProfile: (name: string, params?: ProfileRemoveParams) => Promise<void>;
  clearProfileErrors: () => void;
  fetchLoopStatus: (profile: string, opts?: { force?: boolean }) => Promise<void>;
  startLoop: (profile: string) => Promise<void>;
  stopLoop: (profile: string) => Promise<void>;
}

/** Below this age, a fetch* call reuses the cached value instead of
 * re-requesting -- short enough that "what's running now" stays honest,
 * long enough that switching between Overview/Telemetry/Quota in the same
 * minute doesn't re-hit the CLI three times. */
const FRESH_MS = 15_000;

function isFresh(resource: Resource<unknown>, key: string): boolean {
  return resource.key === key && resource.fetchedAt !== null && Date.now() - resource.fetchedAt < FRESH_MS;
}

function errorMessage(error: unknown): string {
  if (error instanceof GahApiError) return error.message;
  return error instanceof Error ? error.message : String(error);
}

export const useGahStore = create<GahStoreState>((set, get) => ({
  status: emptyResource(),
  report: emptyResource(),
  reportSeries: emptyResource(),
  events: emptyResource(),
  workTimelines: {},
  profiles: emptyResource(),
  loopStatus: emptyResource(),
  loopAction: { pending: false, error: null },
  profileCrud: {
    adding: false,
    updating: false,
    removing: false,
    addError: null,
    updateError: null,
    removeError: null,
    lastAddSuccess: false,
    lastUpdateSuccess: false,
    lastRemoveSuccess: false,
  },

  async fetchStatus(profile, opts) {
    const key = profile ?? '';
    const current = get().status;
    if (current.loading || (!opts?.force && isFresh(current, key))) return;
    set({ status: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getStatus(profile);
      set({ status: { data, loading: false, error: null, fetchedAt: Date.now(), key } });
    } catch (error) {
      set({ status: { ...get().status, loading: false, error: errorMessage(error), key } });
    }
  },

  async fetchReport(params, opts) {
    const key = JSON.stringify(params ?? {});
    const current = get().report;
    if (current.loading || (!opts?.force && isFresh(current, key))) return;
    set({ report: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getReport(params);
      set({ report: { data, loading: false, error: null, fetchedAt: Date.now(), key } });
    } catch (error) {
      set({ report: { ...get().report, loading: false, error: errorMessage(error), key } });
    }
  },

  async fetchReportSeries(params, opts) {
    const key = JSON.stringify(params ?? {});
    const current = get().reportSeries;
    if (current.loading || (!opts?.force && isFresh(current, key))) return;
    set({ reportSeries: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getReportSeries(params);
      set({ reportSeries: { data, loading: false, error: null, fetchedAt: Date.now(), key } });
    } catch (error) {
      set({ reportSeries: { ...get().reportSeries, loading: false, error: errorMessage(error), key } });
    }
  },

  async fetchEvents(params, opts) {
    const key = JSON.stringify(params ?? {});
    const current = get().events;
    if (current.loading || (!opts?.force && isFresh(current, key))) return;
    set({ events: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getEvents(params);
      set({ events: { data, loading: false, error: null, fetchedAt: Date.now(), key } });
    } catch (error) {
      set({ events: { ...get().events, loading: false, error: errorMessage(error), key } });
    }
  },

  async fetchWorkTimeline(workId, opts) {
    const current = get().workTimelines[workId] ?? emptyResource<LedgerEntry[]>();
    if (current.loading || (!opts?.force && isFresh(current, workId))) return;
    set({ workTimelines: { ...get().workTimelines, [workId]: { ...current, loading: true, error: null } } });
    try {
      const data = await gahApi.getWorkTimeline(workId);
      set({
        workTimelines: {
          ...get().workTimelines,
          [workId]: { data, loading: false, error: null, fetchedAt: Date.now(), key: workId }
        }
      });
    } catch (error) {
      set({
        workTimelines: {
          ...get().workTimelines,
          [workId]: { ...current, loading: false, error: errorMessage(error), key: workId }
        }
      });
    }
  },

  async fetchProfiles(opts) {
    const current = get().profiles;
    if (current.loading || (!opts?.force && isFresh(current, ''))) return;
    set({ profiles: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getProfiles();
      set({ profiles: { data, loading: false, error: null, fetchedAt: Date.now(), key: '' } });
    } catch (error) {
      set({ profiles: { ...get().profiles, loading: false, error: errorMessage(error), key: '' } });
    }
  },

  async addProfile(data) {
    set({ profileCrud: { ...get().profileCrud, adding: true, addError: null } });
    try {
      await gahApi.addProfile(data);
      set({ 
        profileCrud: { 
          ...get().profileCrud, 
          adding: false, 
          addError: null, 
          lastAddSuccess: true 
        },
        // Refresh profiles list
        profiles: { ...get().profiles, loading: true }
      });
      // Re-fetch profiles
      try {
        const profilesData = await gahApi.getProfiles();
        set({ 
          profiles: { 
            data: profilesData, 
            loading: false, 
            error: null, 
            fetchedAt: Date.now(), 
            key: '' 
          }
        });
      } catch {
        // If refetch fails, that's okay - the add might still have succeeded
      }
    } catch (error) {
      set({ 
        profileCrud: { 
          ...get().profileCrud, 
          adding: false, 
          addError: errorMessage(error),
          lastAddSuccess: false 
        }
      });
    }
  },

  async updateProfile(name, data) {
    set({ profileCrud: { ...get().profileCrud, updating: true, updateError: null } });
    try {
      await gahApi.updateProfile(name, data);
      set({ 
        profileCrud: { 
          ...get().profileCrud, 
          updating: false, 
          updateError: null, 
          lastUpdateSuccess: true 
        },
        // Refresh profiles list
        profiles: { ...get().profiles, loading: true }
      });
      // Re-fetch profiles
      try {
        const profilesData = await gahApi.getProfiles();
        set({ 
          profiles: { 
            data: profilesData, 
            loading: false, 
            error: null, 
            fetchedAt: Date.now(), 
            key: '' 
          }
        });
      } catch {
        // If refetch fails, that's okay - the update might still have succeeded
      }
    } catch (error) {
      set({ 
        profileCrud: { 
          ...get().profileCrud, 
          updating: false, 
          updateError: errorMessage(error),
          lastUpdateSuccess: false 
        }
      });
    }
  },

  async removeProfile(name, params) {
    set({ profileCrud: { ...get().profileCrud, removing: true, removeError: null } });
    try {
      await gahApi.removeProfile(name, params);
      set({ 
        profileCrud: { 
          ...get().profileCrud, 
          removing: false, 
          removeError: null, 
          lastRemoveSuccess: true 
        },
        // Refresh profiles list
        profiles: { ...get().profiles, loading: true }
      });
      // Re-fetch profiles
      try {
        const profilesData = await gahApi.getProfiles();
        set({ 
          profiles: { 
            data: profilesData, 
            loading: false, 
            error: null, 
            fetchedAt: Date.now(), 
            key: '' 
          }
        });
      } catch {
        // If refetch fails, that's okay - the remove might still have succeeded
      }
    } catch (error) {
      set({ 
        profileCrud: { 
          ...get().profileCrud, 
          removing: false, 
          removeError: errorMessage(error),
          lastRemoveSuccess: false 
        }
      });
    }
  },

  clearProfileErrors() {
    set({
      profileCrud: {
        ...get().profileCrud,
        addError: null,
        updateError: null,
        removeError: null,
        lastAddSuccess: false,
        lastUpdateSuccess: false,
        lastRemoveSuccess: false
      }
    });
  },

  async fetchLoopStatus(profile, opts) {
    const current = get().loopStatus;
    if (current.loading || (!opts?.force && isFresh(current, profile))) return;
    set({ loopStatus: { ...current, loading: true, error: null } });
    try {
      const data = await gahApi.getLoopStatus(profile);
      set({ loopStatus: { data, loading: false, error: null, fetchedAt: Date.now(), key: profile } });
    } catch (error) {
      set({ loopStatus: { ...get().loopStatus, loading: false, error: errorMessage(error), key: profile } });
    }
  },

  async startLoop(profile) {
    set({ loopAction: { pending: true, error: null } });
    try {
      const result = await gahApi.startLoop(profile);
      set({ loopAction: { pending: false, error: result.started ? null : (result.error ?? 'Loop is already running') } });
    } catch (error) {
      set({ loopAction: { pending: false, error: errorMessage(error) } });
    }
    await get().fetchLoopStatus(profile, { force: true });
  },

  async stopLoop(profile) {
    set({ loopAction: { pending: true, error: null } });
    try {
      const result = await gahApi.stopLoop(profile);
      set({ loopAction: { pending: false, error: result.stopped ? null : (result.error ?? 'Failed to stop loop') } });
    } catch (error) {
      set({ loopAction: { pending: false, error: errorMessage(error) } });
    }
    await get().fetchLoopStatus(profile, { force: true });
  }
}));
