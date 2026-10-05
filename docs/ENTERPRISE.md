# Optional enterprise integration

The public GAH core runs without the private enterprise repository. Use a normal clone and the existing build commands for local operation.

The optional `enterprise/` submodule references [Kh1ng/git-agent-harness-enterprise](https://github.com/Kh1ng/git-agent-harness-enterprise). Authorized developers can fetch it with:

```sh
git submodule update --init enterprise
```

GitHub access to the private repository is required. A recursive clone attempts to fetch it; public contributors can use a normal clone instead. Core npm workspaces do not include `enterprise/`.

The enterprise repository contains the managed Supabase integration and separate Markdown implementation tickets. Start with its [Supabase handoff](https://github.com/Kh1ng/git-agent-harness-enterprise/blob/main/docs/tickets/README.md). The regular dashboard build is under review. Pilot publication and commercial license terms remain open.

Keep enterprise source, deployment credentials, node credentials, and provider login files out of public changes and build artifacts. Git submodule access does not grant workspace membership, provider account permission, or a product entitlement.
