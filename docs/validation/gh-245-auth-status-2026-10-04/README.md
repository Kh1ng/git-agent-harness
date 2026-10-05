# gh 2.45.0 auth-status account states (issue #1324)

Date: 2026-10-04.

The auth-status account-state formats relevant to #1324 were reproduced against the
tester's CLI version using the real `gh` 2.45.0 binary (official macOS
arm64 release), never the developer's own credentials: every capture
below ran with an isolated `GH_CONFIG_DIR` containing synthetic config
files with bogus tokens (`ghp_0000…`) and the generic account name
`octo`. The developer's keyring was not read by 2.45.0. No real token or
account name appears anywhere in this proof.

These captures do **not** reproduce the tester's successful-login/failed-setup
discrepancy. Acceptance criterion 4 remains unverified: no confirmed cause of
that discrepancy is established by this proof.

## Captures

**Not logged in** (empty config):

- exit 1, stdout empty
- stderr: `You are not logged into any GitHub hosts. To log in, run: gh auth login`

**Logged in, dead token** (hosts.yml with a bogus token):

- exit 0 — gh 2.45.0 does not fail the command for a rejected credential
- stdout:

```
github.com
  X Failed to log in to github.com account octo (/tmp/gh245/conf-bad/hosts.yml)
  - Active account: true
  - The token in /tmp/gh245/conf-bad/hosts.yml is invalid.
  - To re-authenticate, run: gh auth login -h github.com
  - To forget about this account, run: gh auth logout -h github.com -u octo
```

**Two hosts, both with dead tokens**: still exit 0. A multi-host status
therefore reaches the classifier as a *successful* probe that can carry
both a `✓ Logged in` account line (valid host) and a credential failure
(another host) — the shape the classifier must resolve in favor of the
credential failure.

**Logged in, valid token** (this machine, gh 2.102.0, account name
withheld): exit 0, stdout `✓ Logged in to github.com account … (keyring)`
plus protocol/token/scopes lines, stderr empty. The 2.45.0 `validEntry`
format in `pkg/cmd/auth/status/status.go` (v2.45.0 tag) is the same
shape. Built from this branch, `gah auth-health` reports `gh` state
`ok`, and `gah setup --check --json --role cli-only` reports
`provider_login` state `ok` with `ready: true` — setup detects the
existing login instead of reporting it missing.

## What this proves for the ticket

- gh 2.45.0 reports a dead token on a command that exits 0, so exit
  status alone cannot classify the account state.
- "Not logged in" is a failed command whose text goes to stderr only.
- A slow or killed status check (the 20-second probe budget in
  `src/setup/host.rs`) returns no probe result to setup; even partial child
  output is discarded. It must not be reported as "not logged in" or
  "missing". This is a possible failure mode, not evidence that the tester's
  probe timed out.

## Outstanding diagnosis (review repair, 2026-10-05)

The source issue contract and the Linux audit both say the failing probe's
raw results were absent. The v0.1.3 implementation of `login_status` maps
every classified state other than `Ok` to `NotLoggedIn`, and maps an absent
probe to `Missing`. This confirms loss of diagnostic distinctions in setup;
it cannot establish which input occurred on the tester's machine. Missing
authentication, credential rejection, unrecognized output, command failure
and timeout remain distinct possible explanations. A successful `gh auth
login` alone does not select among them for a later status check.

To close acceptance criterion 4, capture the failing setup probe and a direct
`gh auth status` check in the same environment: resolved executable/version,
exit status or timeout, elapsed time, and sanitized stdout/stderr. Record
whether credential/config overrides (`GH_CONFIG_DIR`, `GH_HOST`, `GH_TOKEN`,
`GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`) are present,
without recording token values. Account names, tokens and identifying paths
must be redacted before publishing. Use the confirmed difference to add a
regression and repeat the scenario with the fix. The existing synthetic
timeout tests verify recovery semantics only; they do not close this criterion.

## Not verified here

- A genuinely *expired* token: gh 2.45.0 prints the same
  `invalidTokenEntry` for any rejected credential, so the bogus-token
  capture above is the same shape it would emit.
- The timeout entry (`X Timeout trying to log in …`): taken from the
  v2.45.0 source (`timeoutErrorEntry`); not reproduced live.
- The `You are not logged into any GitHub Enterprise Server hosts.`
  line used by one defensive regression: no gh between 2.45.0 and
  2.102.0 prints it in `auth status` (checked the tag's `status.go`); it
  is kept so an unrelated "not logged in" message can never mask a
  login line.
- The desktop rows: typechecked (`tsc --noEmit`), not exercised in a
  running app.

## Where the regressions live

- `src/auth_health.rs`: `gh_245_auth_status_account_states`,
  `gh_login_line_does_not_mask_a_credential_failure`.
- `src/setup/requirements.rs`: `gh_245_login_states_stay_distinct`,
  `login_states_serialize_distinctly`.
- `src/setup/wizard.rs`: `a_failed_login_stops_setup_and_says_what_to_do`,
  `an_unconfirmed_login_is_rechecked_not_logged_in_again`.

## The failed `gh auth login` inside setup

The transcript shows setup's own `gh auth login` printing its first menu,
then failing with no visible keypress. The cause was not reproduced. gh
2.45.0 was run the way `SystemEffects::run` runs it: `sh -c` with
inherited stdio, under a pseudo-terminal, right after a line-buffered
`[Y/n]` answer. The menu waited for input. An extra buffered newline or
CR/LF only accepted the defaults, and `GH_TOKEN` in the environment did
not fail the menu either. Without the raw transcript, the remaining
suspects are an interrupt or a terminal quirk on the tester's side.

Either way, setup no longer continues after a failed login. It stops,
names the command to run by hand, and says how to resume. A status check
that fails or is not recognized no longer offers a login at all; setup
says the login may still be valid and to re-run it to re-check.
