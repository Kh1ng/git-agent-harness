# Optional enterprise integration

The public GAH core runs without the private enterprise repository. Use a normal clone and the existing build commands for local operation.

The optional `enterprise/` submodule references [Kh1ng/git-agent-harness-enterprise](https://github.com/Kh1ng/git-agent-harness-enterprise). Authorized developers can fetch it with:

```sh
git submodule update --init enterprise
```

GitHub access to the private repository is required. A recursive clone attempts to fetch it. Public contributors can use a normal clone instead. Core npm workspaces do not include `enterprise/`.

The private repository contains the managed Supabase integration and its implementation tickets. Its [handoff](https://github.com/Kh1ng/git-agent-harness-enterprise/blob/main/docs/tickets/README.md) describes the private work. Public backlog items belong in GitHub issues.

Public core interfaces remain available for independent integrations. Existing local HTTP, live-message, and provider interfaces remain unchanged by this submodule reference.

Keep enterprise source, deployment credentials, node credentials, and provider login files out of public changes and build artifacts. Git submodule access does not grant workspace membership, provider account permission, or a product entitlement.
