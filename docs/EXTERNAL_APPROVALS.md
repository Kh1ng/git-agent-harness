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

When you intend to authorize that request, use the same identifiers:

```sh
gah external-approval grant --profile example --work-id '#42' \
  --credential-label example-service --operation-kind external_api --json
```

Omitted grant limits, expiry, and purpose inherit the pending request. Explicit limits may be smaller. A grant cannot extend the requested expiry or change its purpose.

A grant requires a pending request. Repeated grants fail instead of resetting consumption. To renew an approval, record and inspect a new request first.

An expiry must use RFC3339 and remain in the future. Request counts must be positive integers. Dollar limits must be finite and positive.

Revoke with the same identifiers:

```sh
gah external-approval revoke --profile example --work-id '#42' \
  --credential-label example-service --operation-kind external_api --json
```

Revocation retains the original scope and consumption in inspection. An external grant does not release an unrelated human hold.

## Current limits

The counter records completed backend attempts, including failed attempts. It does **not** measure individual service requests or stop requests inside a running backend.

External-service dollar usage is unknown. A dollar-capped grant becomes unavailable for the next attempt after consumption with unknown usage. Backend token costs remain separate.

These controls restrict credential injection and subsequent attempts. They are not a service-side spending limit. Pending requests do not yet provide a dedicated dispatch hold or automatic resume workflow.

Issue #653 tracks dedicated holds, notifications, authenticated dashboard controls, and service-request accounting. Existing ledger history remains readable; inspection and credential injection use the same approval state.
