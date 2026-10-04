/**
 * Small UI-preference store: theme, navigation preferences, and an optional
 * profile override.
 *
 * The WebSocket provider reconnects with the selected profile so live status
 * and provider data follow the same profile as the REST-backed pages.
 */
import { create } from 'zustand';
import { readNavigation } from '../lib/navigationState.js';

export type Theme = 'dark' | 'light';

interface UiStoreState {
  theme: Theme;
  profileOverride: string | null;
  /** Keep Nodes in the navbar on a standalone install (no worker nodes). */
  showNodes: boolean;
  setShowNodes: (show: boolean) => void;
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
  profileOverride: typeof window === 'undefined' ? null : readNavigation().profile,
  showNodes: typeof window !== 'undefined' && window.localStorage.getItem('gah-show-nodes') === 'true',
  setShowNodes: (showNodes) => {
    if (typeof window !== 'undefined') window.localStorage.setItem('gah-show-nodes', String(showNodes));
    set({ showNodes });
  },
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
