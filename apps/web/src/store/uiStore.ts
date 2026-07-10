/**
 * Small UI-preference store: theme and an optional profile override.
 *
 * The WebSocket provider reconnects with the selected profile so live status
 * and provider data follow the same profile as the REST-backed pages.
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
