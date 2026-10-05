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
  /** Pop a new notification open under the bell for a few seconds. */
  notificationPopups: boolean;
  setNotificationPopups: (enabled: boolean) => void;
  setTheme: (theme: Theme) => void;
  setProfileOverride: (profile: string | null) => void;
  /** The project switcher asked for a form another view owns: the Projects page's import, or the Profile sidebar's add. */
  pendingAction: 'import' | 'create' | null;
  requestAction: (action: 'import' | 'create' | null) => void;
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
  notificationPopups: typeof window === 'undefined' || window.localStorage.getItem('gah-notification-popups') !== 'false',
  setNotificationPopups: (notificationPopups) => {
    if (typeof window !== 'undefined') window.localStorage.setItem('gah-notification-popups', String(notificationPopups));
    set({ notificationPopups });
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
  setProfileOverride: (profile) => set({ profileOverride: profile }),
  pendingAction: null,
  requestAction: (pendingAction) => set({ pendingAction })
}));
