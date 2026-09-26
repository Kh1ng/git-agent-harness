# Worker role and central connection

The CLI stores the host role in `[defaults].node_role`. Existing configurations default to `central`.

```sh
gah config set --node-role worker --registry-central-url https://central.example.test
gah status --role --json
```

The worker requires a central HTTP(S) origin and `COORDINATOR_TOKEN`. The URL must not contain credentials, a query, or a path.
An explicit `GAH_NODE_ROLE` overrides the stored role for that process. Remove the override before you change roles through configuration.
Restart the running server or worker service after a role change. The HTTP health response reports the role that the running server uses.

Workers permit local execution, status, and readiness requests. They reject central registry, claims, settings, skills, and manager-chat requests.
They do not read a local central registry, initialize a claims store, seed the skill bank, or start central maintenance schedulers.
The existing WebSocket session protocol still supports remote execution.

Worker dispatch capture uses central's authenticated `/api/worker-memory` relay. Central owns the gateway credential and checks its memory settings for the requested profile.
The relay accepts only recall, capture, and session-end operations. Clients must provide `profile` and the existing operation fields.
A disabled profile skips gateway access. An unavailable gateway does not block dispatch.
Dispatch recall runs through the backend memory hooks that `gah setup memory-hooks` installs (#830). Rust recall has no production dispatch caller.

For Linux or macOS worker setup, supply `GAH_CENTRAL_URL` and `COORDINATOR_TOKEN` when you run the installer.
The installer saves the token in `~/.config/gah/gah-loop.env` with mode `0600`.
A reinstall can use the saved token.

The macOS desktop controls a worker server on port 3774.
By default, the server binds to the Mac tailnet IPv4 address.
You can use Tailscale Serve, an explicit URL, or a managed reverse SSH tunnel instead.
The installer saves the selected URL and transport in the worker LaunchAgent.
An update uses these saved values unless you supply new values.
The worker registers all configured profiles after startup.
The installer asks central to poll the registered worker before it reports success.
Central can then import projects and send chat work to the Mac.
The desktop worker control does not start `gah loop`.

Configure the gateway on central. Worker installers reject `GAH_GATEWAY_MODE`.
Run project memory imports on central.

The WSL installer needs a matching CLI release with `config set --node-role` and `status --role` support.
An older downloaded CLI fails its capability check before the installer changes the active worker settings or service files.
Publishing that matching release and testing Windows installation remain separate steps.
Native Windows execution, mobile controls, and macOS central-service installation are outside this change.
