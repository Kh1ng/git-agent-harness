# External approval state

An external approval names one profile, repository, work item, credential label, and operation. Work identifiers match exactly, including GitLab URLs on custom domains.

## Request and grant

Configure the credential label under the profile's `external_credential_scopes`. Keep credential values out of commands and approval purposes.

Record a request with its intended bounds and purpose. The following example records local approval metadata only:

```sh
gah external-approval request --profile example --work-id '#42' \
  --credential-label example-service --operation-kind external_api \
  --max-requests 1 --purpose 'Check the requested service'
```

Inspect the pending request before granting it:

```sh
gah external-approval inspect --profile example --work-id '#42' \
  --credential-label example-service --operation-kind external_api --json
```

List all approval scopes for a profile:

```sh
gah external-approval list --profile example --json
```

When you intend to authorize that request, use the same identifiers:

```sh
gah external-approval grant --profile example --work-id '#42' \
  --credential-label example-service --operation-kind external_api --json
```

Omitted grant limits, expiry, and purpose inherit the pending request. Explicit limits may be smaller. A grant cannot extend the requested expiry or change its purpose.

A grant requires a pending request. Repeated grants fail instead of resetting consumption. To renew an approval, record and inspect a new request first.

Deny a pending request with the same identifiers:

```sh
gah external-approval deny --profile example --work-id '#42' \
  --credential-label example-service --operation-kind external_api --json
```

An expiry must use RFC3339 and remain in the future. Request counts must be positive integers. Dollar limits must be finite and positive.

Revoke with the same identifiers:

```sh
gah external-approval revoke --profile example --work-id '#42' \
  --credential-label example-service --operation-kind external_api --json
```

Revocation retains the original scope and consumption in inspection. An external grant does not release an unrelated human hold.

## Automatic pause and notification

GAH pauses one work item before backend launch when its credential scope has no active grant.

The notification includes the project, work link, credential label, bounds, expiry, reason, and exact grant and deny commands.

The same pending request produces one notification. A restart does not produce a second notification.

Open the Work page to review pending requests. An authenticated owner can grant, deny, or revoke the exact recorded scope.

A grant makes the work item eligible for the next loop cycle. A denial, expiry, or exhausted cap keeps the item paused.

## Current limits

The counter records completed backend attempts, including failed attempts. It does **not** measure individual service requests or stop requests inside a running backend.

External-service dollar usage is unknown. A dollar-capped grant becomes unavailable for the next attempt after consumption with unknown usage. Backend token costs remain separate.

These controls restrict credential injection and subsequent attempts. They are not a service-side spending limit.

The ledger remains the source of truth. The CLI, server, dashboard, dispatch gate, and notification path use the same approval state.
