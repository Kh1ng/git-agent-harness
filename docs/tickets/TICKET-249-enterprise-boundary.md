# TICKET-249: Keep enterprise integration outside the public core

Status: Repository separation implemented. Private runtime packaging is under review. License audit and commercial terms remain open.

Keep local GAH usable without a cloud account or enterprise license. Put the supplied enterprise connection and its administration features in a separate private repository with separate licensing.

The private repository is [Kh1ng/git-agent-harness-enterprise](https://github.com/Kh1ng/git-agent-harness-enterprise), referenced by the optional `enterprise/` submodule. Supabase implementation tickets and the agent handoff are in its [ticket index](https://github.com/Kh1ng/git-agent-harness-enterprise/blob/main/docs/tickets/README.md). The core stores a URL and commit reference, not enterprise source.

Public core requirements:

- Preserve existing local HTTP, live-message, and provider interfaces.
- Add a neutral connection contract only when needed by the separately packaged adapter. Do not introduce a Supabase dependency into the core.
- Keep configuration export/import neutral so a local installation can move or use an optional connection. Export secret references separately from secret values.
- Allow simultaneous local and optional hosted operation with explicit routing, shared capacity admission, and stable job identities. Do not execute a job twice or silently bypass a rejected hosted request through local fallback.
- Preserve existing code licenses. Audit package declarations and establish the root license before changing distribution terms.
- Exclude local enterprise staging, private deployment state, credentials, and provider login files from public changes and build artifacts.

The supplied enterprise source can use separate commercial terms. The public interfaces remain available for independent integrations.

Verification for this separation: confirm enterprise draft files are ignored and untracked, inspect the public diff, and check that the local server and dashboard still build without Supabase dependencies.
