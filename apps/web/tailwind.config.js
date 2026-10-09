import plugin from 'tailwindcss/plugin';

/** @type {import('tailwindcss').Config} */
export default {
  // `dark:` follows the theme's scheme (src/lib/themes.ts), not its id, so every dark theme gets it.
  darkMode: ['selector', '[data-scheme="dark"]'],
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        page: 'rgb(var(--surface-page) / <alpha-value>)',
        card: 'rgb(var(--surface-card) / <alpha-value>)',
        raised: 'rgb(var(--surface-raised) / <alpha-value>)',
        subtle: 'rgb(var(--border-subtle) / <alpha-value>)',
        primary: 'rgb(var(--ink-primary) / <alpha-value>)',
        secondary: 'rgb(var(--ink-secondary) / <alpha-value>)',
        muted: 'rgb(var(--ink-muted) / <alpha-value>)',
        accent: 'rgb(var(--accent) / <alpha-value>)',
        // Solid fills; text on them is `on-fill` (4.5:1 in every theme). `accent` alone is for text and outlines.
        'accent-fill': 'rgb(var(--accent-fill) / <alpha-value>)',
        'critical-fill': 'rgb(var(--critical-fill) / <alpha-value>)',
        'on-fill': 'rgb(var(--on-fill) / <alpha-value>)',
        // Tint for hover, pressed and track states, used at low alpha (bg-overlay/5): lightens a dark theme, darkens a light one.
        overlay: 'rgb(var(--overlay) / <alpha-value>)',
        // Modal backdrop.
        scrim: 'rgb(var(--scrim) / <alpha-value>)',
        good: 'rgb(var(--status-good) / <alpha-value>)',
        warning: 'rgb(var(--status-warning) / <alpha-value>)',
        serious: 'rgb(var(--status-serious) / <alpha-value>)',
        critical: 'rgb(var(--status-critical) / <alpha-value>)',
        unknown: 'rgb(var(--status-unknown) / <alpha-value>)',
        'series-1': 'rgb(var(--series-1) / <alpha-value>)',
        'series-2': 'rgb(var(--series-2) / <alpha-value>)',
        'series-3': 'rgb(var(--series-3) / <alpha-value>)',
        'series-4': 'rgb(var(--series-4) / <alpha-value>)',
        'series-5': 'rgb(var(--series-5) / <alpha-value>)',
        'series-6': 'rgb(var(--series-6) / <alpha-value>)',
        'series-7': 'rgb(var(--series-7) / <alpha-value>)',
        'series-8': 'rgb(var(--series-8) / <alpha-value>)',
      },
      borderColor: {
        DEFAULT: 'rgb(var(--border-subtle) / 1)',
      },
      fontFamily: {
        sans: ['system-ui', '-apple-system', '"Segoe UI"', 'sans-serif'],
      },
    },
  },
  plugins: [
    require('@tailwindcss/typography'),
    // Chat lays itself out by the room it has, not the window: docked in the
    // right sidebar (`.chat-docked`) it is narrow however wide the window is.
    plugin(({ addVariant }) => {
      addVariant('wide', '@media (min-width: 1280px) { &:not(.chat-docked *) }');
      addVariant('narrow', ['@media not all and (min-width: 1280px) { & }', '.chat-docked &']);
    }),
  ],
}
