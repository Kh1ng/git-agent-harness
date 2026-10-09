# Theme audit, 2026-10-09

Scope: the web dashboard (`apps/web`), which the desktop app also renders.
The question was why the themes had drifted apart and what it takes to keep
several themes in step. Findings come from reading the source and computing
WCAG contrast from the token values. The component test
`Navigation.spec.tsx` now repeats the contrast checks for every theme.

## What was there

The app shipped two themes, Dark and Light, as two blocks of CSS custom
properties in `src/index.css`. No Gruvbox theme existed in this repository,
in the enterprise overlay, on any branch or in history.

The token layer itself was sound: surfaces, ink, status, accent and chart
series were already semantic, and most components used them. The drift came
from the places that bypassed the tokens. Each one was correct in the theme
its author was looking at and wrong in the other.

## Findings

| # | Severity | Finding | Fix |
|---|---|---|---|
| T1 | P1 | Hover and pressed states used `hover:bg-white/5` (64 sites in components plus `.nav-link`, `.top-nav-link`, `.activity-bar-button`, `.btn-secondary`, `.chat-menu-item`). White at 5% over the light card (#fcfcfb) changes nothing, so in Light no list row, menu item or secondary button showed hover feedback. | New `--overlay` token: white in Dark, near-black in Light, fg1 in Gruvbox. Every site is now `bg-overlay/5`. |
| T2 | P1 | `AgentLimitsSection` "Save agent settings" painted white text on `bg-accent` (2.86:1 in Dark), the pattern #1480 removed elsewhere but missed here. | Uses `btn-primary`. |
| T3 | P1 | `ProfileEditor` error and success banners used `bg-red-50` / `bg-green-50` with `text-red-700` / `text-green-700`: a pale pink or mint slab on the dark card, styled for a light page only. The delete confirmation button used `bg-red-600` / `hover:bg-red-700`. | Banners use `critical` / `good` tints. New `btn-danger` over a `--critical-fill` token. |
| T4 | P2 | Status text hard-coded to the Tailwind palette: `text-red-400`, `text-red-500`, `text-green-600`, `text-amber-300`, `text-emerald-400`, `fill-amber-400` (favorites), `text-purple-300/400` (PR strip). The 400 shades are tuned for dark backgrounds and fail 4.5:1 on Light; the 600/700 shades the reverse. | Mapped to `critical`, `good`, `warning`, and `series-5` for the PR strip. |
| T5 | P2 | Text on solid fills was hard-coded `text-white` (13 sites). That only works while every theme's fill is dark enough for white, which is not true of Gruvbox, whose fills take dark text. | New `--on-fill` token used by `btn-primary`, `btn-danger` and every `bg-accent-fill` chip. |
| T6 | P2 | The Light token block was written twice, once under `[data-theme='light']` and once under `@media (prefers-color-scheme: light)`. The second copy could never apply in the app, because `main.tsx` sets `data-theme` before first paint and the stored default is Dark, but it had to be kept in sync by hand. | Removed. `:root` carries the Dark tokens as the fallback. |
| T7 | P2 | The theme list was spread over five places: the `Theme` union in `uiStore.ts`, the localStorage guard, two hand-written buttons in `SettingsPage.tsx`, the Tailwind `darkMode` selector (`[data-theme="dark"]`, so any second dark theme would silently lose `dark:` styles), and the fixed `#0d0d0d` browser chrome color. Adding a theme meant finding all five. | One registry, `src/lib/themes.ts`. `ThemePicker` renders from it, `applyTheme` sets `data-theme`, `data-scheme` (which `dark:` now follows) and the chrome color. |
| T8 | P3 | Modal backdrops used `backdrop:bg-black/35..70` and the agent log well `bg-black/20`; inline code in chat used `rgb(0 0 0 / 0.2)`. | `bg-scrim/NN`, `bg-page`, and `--overlay` at 10%. |
| T9 | P3 | `--border-hairline` was defined in both themes and used nowhere. | Removed; `--overlay` covers the same need. |
| T10 | Note | The session preview iframe keeps `bg-white`. It is the canvas of an arbitrary web page, which browsers default to white, so it is deliberately not themed. | None. |

## Out of this change

| # | Where | Finding |
|---|---|---|
| O1 | Enterprise overlay | `integration/overlay/apps/web/src/pages/SettingsPage.tsx` is a full copy of core's Settings page pinned to an older core revision. It still has the hand-written Dark/Light toggle with white on `bg-accent` (2.86:1) and four more white-on-accent buttons. Once the overlay's core pin moves past this change, its Appearance section can render `<ThemePicker />` and pick up Gruvbox. |
| O2 | Android | `values/themes.xml` and `values-night/themes.xml` use an indigo accent (#4F46E5 / #818CF8) unrelated to the web accent, and have no Gruvbox variant. |
| O3 | iOS | Uses system colors only; there is no app theme to align. |

## Gruvbox

Gruvbox dark, hard contrast: page `bg0_h`, card `bg0`, raised `bg0_s`,
borders `bg2`, ink `fg1`/`fg2`. Status and accent use the Gruvbox bright
hues. Two published tones fall short of 4.5:1 for small text and were
lightened only as far as needed:

| Token | Gruvbox | Used | Worst case before | After |
|---|---|---|---|---|
| `--status-critical` | red #fb4934 | #fc8071 | 3.63:1 on its badge tint, 3.82:1 on raised | 4.61:1 |
| `--status-serious` | orange #fe8019 | #fe8d30 | 4.34:1 on its badge tint | 4.60:1 |

Muted ink stays fg4 (#a89984), 4.72:1 on the raised surface.

Fills (`accent-fill` blue #83a598, `critical-fill` red #fb4934) take dark
text (`--on-fill` #1d2021): 6.09:1 and 4.77:1.

## Adding a theme

1. Add an entry to `THEMES` in `src/lib/themes.ts`.
2. Add a `[data-theme='<id>']` block to `src/index.css` with every token the
   Dark block defines.
3. Run `npm run test:component --workspace=apps/web`; the contrast test
   covers the new theme without changes.

Components need no change, provided they keep to the token classes. A
palette class such as `text-red-400` or `bg-white/5` in a component is the
thing to flag in review.
