# Process resource telemetry

GAH records host consumption separately from provider tokens, prices and quotas.
Each ledger attempt and version 10 telemetry export includes `resources`:

- `cpu_seconds`: sum of observed user and system CPU time per process identity.
- `peak_rss_bytes`: largest observed simultaneous RSS sum across the process tree.
- `source`: `linux_procfs_sampled_lower_bound` or `unavailable`.
- `unknown_reason`: why measurements are unavailable. Historical rows use `not_recorded`.

Linux observations include the root, process group, descendants that create a new
session, and previously observed descendants that become reparented. PID start
times distinguish reused IDs. Observed CPU maxima remain after children exit.
Success, nonzero exit, timeout and graceful cancellation retain observations.
Native unsupported platforms report null values with `unsupported_platform`.
WSL workers use the Linux sampler.

These are sampled lower bounds, not complete lifetime accounting. Dispatch polls
at 500 ms; review sampling runs at most every 250 ms. Processes that start and
exit between scans, inaccessible processes and intermediate memory peaks can be
missed. RSS can count shared pages in more than one process. Forced termination
of GAH itself cannot flush an unwritten ledger entry. No command arguments,
environment contents or process names are recorded in resource telemetry.

Use `gah telemetry aggregate --dimensions project,ticket,execution_type,backend_instance,model,outcome,date_range --include-failed-attempts --include-retried-attempts --json`.
Apply the existing `--since` and `--until` filters to bound the report.
CPU aggregates sum known attempts; RSS aggregates take the largest attempt peak.
Null totals mean no observations. Coverage counters and provenance maps preserve
unknown attempts, including historical rows. Resource consumption does not imply
provider spending or token consumption.
