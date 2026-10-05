# gh 2.45.0 auth-status account states (issue #1324)

Date: 2026-10-04.

The setup false "not logged in" report (#1324) was reproduced against the
tester's CLI version using the real `gh` 2.45.0 binary (official macOS
arm64 release), never the developer's own credentials: every capture
below ran with an isolated `GH_CONFIG_DIR` containing synthetic config
files with bogus tokens (`ghp_0000…`) and the generic account name
`octo`. The developer's keyring was not read by 2.45.0. No real token or
account name appears anywhere in this proof.

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
  `src/setup/host.rs`) yields no output at all; it must not be reported
  as "not logged in" or "missing" — that is the leading hypothesis for
  the tester's false logged-out reading.

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
