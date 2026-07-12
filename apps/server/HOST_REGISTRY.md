# Host Registry Configuration

The Host Registry enables multi-host support in the Git Agent Harness server, allowing a dashboard to manage and monitor multiple GAH instances.

## Configuration

### Environment Variables

- `HOST_ID`: Unique identifier for this host (defaults to hostname)
- `HOST_NAME`: Display name for this host (defaults to HOST_ID)
- `GAH_HOSTS_CONFIG`: Path to the host registry configuration file (defaults to `apps/server/hosts.json`)

### Configuration File Format

The host registry configuration file (`hosts.json` by default) uses the following format:

```json
{
  "hosts": [
    {
      "id": "host1",
      "name": "Primary Host",
      "base_url": "http://localhost:3773",
      "auth_token": "optional-auth-token",
      "profile": "gah"
    },
    {
      "id": "host2",
      "name": "Secondary Host",
      "base_url": "http://backup-server:3773",
      "profile": "worldcup"
    }
  ]
}
```

### Configuration Fields

- `id` (string, required): Unique identifier for the host
- `name` (string, required): Display name for the host
- `base_url` (string, required): Base URL of the host's GAH server
- `auth_token` (string, optional): Authentication token for accessing the host's API
- `profile` (string, required): Default profile to use for this host

## API Endpoints

### GET /api/hosts

Returns a list of all configured hosts with their health status:

```json
[
  {
    "id": "host1",
    "name": "Primary Host",
    "base_url": "http://localhost:3773",
    "profile": "gah",
    "reachable": true,
    "latency_ms": 15,
    "status": "healthy"
  },
  {
    "id": "host2",
    "name": "Secondary Host",
    "base_url": "http://backup-server:3773",
    "profile": "worldcup",
    "reachable": false,
    "error": "Connection refused"
  }
]
```

### GET /api/hosts/:id/health

Returns the health status of a specific host:

```json
{
  "host_id": "host1",
  "reachable": true,
  "latency_ms": 15,
  "status": "healthy"
}
```

Or for an unreachable host:

```json
{
  "host_id": "host2",
  "reachable": false,
  "error": "Connection refused"
}
```

### GET /api/info

The server info endpoint now includes host information:

```json
{
  "name": "Git Agent Harness",
  "version": "0.1.0",
  "host_id": "my-hostname",
  "host_name": "my-hostname",
  "features": {
    "multiHostRegistry": true
  }
}
```

## Readiness Check

The host registry includes a readiness check named `hostRegistry` that verifies the configuration can be loaded successfully. This check is visible in the `/health` endpoint.

## Usage

1. Configure the `hosts.json` file with your host information
2. Set environment variables if needed (HOST_ID, HOST_NAME, GAH_HOSTS_CONFIG)
3. Start the server - it will automatically load the host registry
4. Use the `/api/hosts` and `/api/hosts/:id/health` endpoints to monitor hosts

## Notes

- The local host (where the server is running) is always included in the host list
- Host health checks are performed by probing the `/health` endpoint of each host
- Authentication tokens are optional and used for accessing remote hosts
- The host registry is not critical for server operation - if it fails to load, the server will continue to run with an empty registry