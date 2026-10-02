const pages = ['overview', 'work', 'telemetry', 'quota', 'events', 'settings', 'chat', 'projects', 'git', 'nodes', 'planning'] as const;
export type Page = typeof pages[number];
export const DEFAULT_CONVERSATION_ID = 'default';

/** `epic` is the issue number the Planning page maps; `map` a `.plan/maps/` slug instead. */
type NavigationState = { page: Page; profile: string | null; chat: string | null; epic: string | null; map: string | null };

/** URLs restore a control surface and conversation, never credentials or commands. */
export function readNavigation(search = window.location.search): NavigationState {
  const params = new URLSearchParams(search);
  const profile = params.get('profile');
  const chat = params.get('chat');
  const epic = params.get('epic');
  const map = params.get('map');
  const validProfile = profile && profile.trim() === profile && profile.length <= 512 && !/[\x00-\x1f\x7f]/.test(profile) ? profile : null;
  return {
    page: pages.find(page => page === params.get('page')) ?? 'overview',
    profile: validProfile,
    chat: validProfile && chat && /^[a-zA-Z0-9_-]{1,128}$/.test(chat) ? chat : null,
    epic: validProfile && epic && /^[1-9][0-9]{0,9}$/.test(epic) ? epic : null,
    map: validProfile && map && /^[a-z0-9][a-z0-9-]{0,99}$/.test(map) ? map : null
  };
}

/** Preserve pairing fragments and other query parameters without adding history entries. */
export function updateNavigation(update: Partial<NavigationState>): void {
  const url = new URL(window.location.href);
  const next = { ...readNavigation(), ...update };
  for (const key of ['page', 'profile', 'chat', 'epic', 'map'] as const) {
    if (next[key]) url.searchParams.set(key, next[key]);
    else url.searchParams.delete(key);
  }
  if (url.href !== window.location.href) window.history.replaceState(window.history.state, '', url);
}

let openedActivity: string | null | undefined;

/** The notification a push or feed link opened (`event=<id>`). The parameter
 * is removed so later navigation doesn't carry it along; repeat calls in the
 * same page load return the same id. */
export function takeActivityDeepLink(): string | null {
  if (openedActivity !== undefined) return openedActivity;
  const url = new URL(window.location.href);
  const id = url.searchParams.get('event');
  if (id !== null) {
    url.searchParams.delete('event');
    window.history.replaceState(window.history.state, '', url);
  }
  openedActivity = id && id.length <= 256 && !/[\x00-\x1f\x7f]/.test(id) ? id : null;
  return openedActivity;
}
