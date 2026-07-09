/**
 * Small UI-preference store: theme and an optional profile override.
 *
 * Honest limitation (see Settings page): the WebSocket welcome message's
 * live session/provider data is tied to whatever profile the server
 * hardcodes at connect time (currently always "gah" -- see
 * apps/server/src/wsServer.ts's `defaultProfile`). Overriding the profile
 * here only affects the REST-backed pull data (status/report/events/work),
 * which genuinely does re-fetch fresh per profile. True multi-profile live
 * session switching would need a WS message to request a different
 * profile's welcome payload, which doesn't exist yet -- rather than fake
 * it, this is called out directly in the Settings UI.
 */
import { create } from 'zustand';

export type Theme = 'dark' | 'light';

interface UiStoreState {
  theme: Theme;
  profileOverride: string | null;
  setTheme: (theme: Theme) => void;
  setProfileOverride: (profile: string | null) => void;
}

function initialTheme(): Theme {
  if (typeof window === 'undefined') return 'dark';
  const stored = window.localStorage.getItem('gah-theme');
  if (stored === 'light' || stored === 'dark') return stored;
  return 'dark';
}

export const useUiStore = create<UiStoreState>((set) => ({
  theme: initialTheme(),
  profileOverride: null,
  setTheme: (theme) => {
    if (typeof document !== 'undefined') {
      document.documentElement.setAttribute('data-theme', theme);
    }
    if (typeof window !== 'undefined') {
      window.localStorage.setItem('gah-theme', theme);
    }
    set({ theme });
  },
  setProfileOverride: (profile) => set({ profileOverride: profile })
}));
