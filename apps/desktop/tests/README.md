The onboarding browser test runs the production Settings page with a mocked
Tauri host. It verifies explicit choices, a failed install followed by retry,
local dashboard connection, missing-package login gating, factory profile
configuration, and terminal-free Linux setup with macOS/Windows terminal
fallback. It does not validate native installation, PolicyKit,
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
Factory service lifecycle, network pairing and local embedding-provider configuration remain
tracked separately in #1317, #1318 and #1319; bundled release installation is
tracked in #1321.

## Repair validation — 2026-10-06

This worker environment is headless and has no PolicyKit authentication agent,
`pkexec`, Xvfb, or GTK development package. It cannot supply acceptance criterion
7's fresh native Linux installation and sanitized GUI failure/retry evidence.
That criterion remains open; mocked screenshots must not be substituted for it.
No PR was created or updated during this repair.

- `npm run --workspace=apps/desktop typecheck`: passed.
- `npm run --workspace=apps/desktop pretest`: production Vite build passed.
- `cargo test --test desktop_onboarding`: 3 passed, including factory argument
  validation and rejection of unsafe TOML values.
- `cargo test setup::`: 27 setup tests passed.
- `cargo fmt --check`: passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `npm run --workspace=apps/desktop test`: browser launch blocked by missing
  `libnspr4.so`; the expanded factory/platform browser checks were not executed.
- `cargo test --manifest-path apps/desktop/Cargo.toml`: build blocked by missing
  `dbus-1.pc` / D-Bus development libraries; native code was not validated.
- `cargo test`: library suite finished with 1,925 passed and 8 failed in
  dispatch/routing reservation and status tests. Cargo stopped before the
  remaining integration suites; the full suite is not green.
- `npm run test:server`: 34 test files passed, 60 failed, including after building
  contracts/shared. Direct ACP integration reproduction reported connection
  closures; the repair does not change ACP code.
- `npm run --workspace=apps/server test:mock`: blocked by sandbox `listen EPERM`
  on tsx's Unix socket. Running via `node --import tsx --test` also failed.

Factory selection now exposes a profile configuration action using `gah init`,
with repository host, repository and checkout path supplied in the app. Existing
profiles are protected by the CLI's duplicate-profile refusal. Configuration does
not enable dispatch. macOS and Windows keep their existing Terminal/PowerShell
setup entry point; Windows executes GAH in the saved WSL distribution.
