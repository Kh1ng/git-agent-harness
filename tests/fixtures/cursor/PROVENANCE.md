# Cursor fixture provenance

Issue #1229.

| Fixture | Source | Notes |
|---|---|---|
| `result_documented_shape.json` | **Not a live capture.** Written by hand from the field list Cursor documents for `cursor-agent -p --output-format json` (`type`, `subtype`, `is_error`, `duration_ms`, `duration_api_ms`, `result`, `session_id`, `request_id`). | Placeholder values throughout; both ids are zeroed. No account identifiers. |

`cursor-agent` was not installed or logged in on the host where the backend
was implemented (#1229 is blocked on `cursor-agent login`), so no real
response could be captured. Replace this fixture with a captured response
from a real run, with ids scrubbed, and update this file. Until then, treat
`runner::backends::cursor::parse_output` as unverified against real output --
in particular the `usage` object keys it accepts are not confirmed by a
capture.
