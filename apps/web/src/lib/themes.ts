/**
 * The themes the app ships. A theme is one entry here plus one
 * `[data-theme='<id>']` block of color tokens in index.css. Components use
 * only the semantic token classes (bg-card, text-muted, hover:bg-overlay/5,
 * text-on-fill, ...), so adding or changing a theme touches no component.
 */
import { Flame, Moon, Sun, type LucideIcon } from 'lucide-react';

export interface ThemeDefinition {
  id: string;
  label: string;
  icon: LucideIcon;
  /** Light or dark as far as the browser is concerned: native controls and the `dark:` variant follow it. */
  scheme: 'dark' | 'light';
  /** Browser chrome color (`<meta name="theme-color">`); matches the theme's page surface. */
  chrome: string;
}

export const THEMES = [
  { id: 'dark', label: 'Dark', icon: Moon, scheme: 'dark', chrome: '#0d0d0d' },
  { id: 'light', label: 'Light', icon: Sun, scheme: 'light', chrome: '#f9f9f7' },
  { id: 'gruvbox', label: 'Gruvbox', icon: Flame, scheme: 'dark', chrome: '#1d2021' },
] as const satisfies readonly ThemeDefinition[];

export type Theme = (typeof THEMES)[number]['id'];

export const DEFAULT_THEME: Theme = 'dark';

export function isTheme(value: unknown): value is Theme {
  return THEMES.some((theme) => theme.id === value);
}

/** Point the document at `theme`'s tokens. */
export function applyTheme(theme: Theme): void {
  if (typeof document === 'undefined') return;
  const definition = THEMES.find((entry) => entry.id === theme) ?? THEMES[0];
  const root = document.documentElement;
  root.setAttribute('data-theme', definition.id);
  root.setAttribute('data-scheme', definition.scheme);
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', definition.chrome);
}
