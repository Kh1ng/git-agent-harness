/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ['selector', '[data-theme="dark"]'],
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
  plugins: [],
}
