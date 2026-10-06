The onboarding browser test runs the production Settings page with a mocked
Tauri host. It verifies explicit choices, a failed install followed by retry,
local dashboard connection, missing-package login gating, and absence of
terminal setup calls. It does not validate native installation, PolicyKit,
provider credentials, or a fresh Linux desktop.

Run from the repository root after `npm ci` and the contracts/shared builds:

```sh
npm run --workspace=apps/desktop typecheck
npm run --workspace=apps/desktop test
cargo test --test desktop_onboarding
cargo test setup::
cargo test --manifest-path apps/desktop/Cargo.toml
```

Playwright needs its Chromium browser and system libraries. Set
`GAH_GUI_EVIDENCE_DIR` to an absolute writable directory when running the browser
test to save sanitized screenshots of its mocked failure and successful retry.

Release validation additionally needs a fresh Linux desktop session with a
PolicyKit authentication agent: exercise missing packages, rejected repository
credentials, coding-agent browser/device login, denied installation permission,
retry, and opening the loopback dashboard without Tailscale or another node.
Verify shared memory both skipped and connected to a test gateway. Screenshots
from a real session must omit tokens, keys, account identifiers and login codes.
Factory, network pairing and local embedding-provider configuration remain
tracked separately in #1317, #1318 and #1319; bundled release installation is
tracked in #1321.
