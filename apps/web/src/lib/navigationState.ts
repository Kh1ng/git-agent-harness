const pages = ['overview', 'work', 'telemetry', 'quota', 'events', 'settings', 'chat', 'git', 'nodes'] as const;
export type Page = typeof pages[number];

type NavigationState = { page: Page; profile: string | null; chat: string | null };

/** URLs restore a control surface and conversation, never credentials or commands. */
export function readNavigation(search = window.location.search): NavigationState {
  const params = new URLSearchParams(search);
  const profile = params.get('profile');
  const chat = params.get('chat');
  const validProfile = profile && profile.trim() === profile && profile.length <= 512 && !/[\x00-\x1f\x7f]/.test(profile) ? profile : null;
  return {
    page: pages.find(page => page === params.get('page')) ?? 'overview',
    profile: validProfile,
    chat: validProfile && chat && /^[a-zA-Z0-9_-]{1,128}$/.test(chat) ? chat : null
  };
}

/** Preserve pairing fragments and other query parameters without adding history entries. */
export function updateNavigation(update: Partial<NavigationState>): void {
  const url = new URL(window.location.href);
  const next = { ...readNavigation(), ...update };
  for (const key of ['page', 'profile', 'chat'] as const) {
    if (next[key]) url.searchParams.set(key, next[key]);
    else url.searchParams.delete(key);
  }
  if (url.href !== window.location.href) window.history.replaceState(window.history.state, '', url);
}
