# Node Secret Rotation and Revocation

This document describes how to rotate credentials and revoke registered nodes in the Git Agent Harness (GAH) coordinator-node network.

## Stable Node Identity
Each node is registered under a stable, non-secret `node_id`. This ID is used as the primary lookup key for all node settings, secret rotation, and registry revocation operations.

## 1. Secret Rotation

GAH implements a **secrets-by-reference** policy. Registry configuration files (`registry-config.json`) must store secret references (e.g. env variables or cert files) instead of raw secret values.

### Supported Secret Reference Formats:
1. **Environment Variables**: Prefix `env:`, followed by the environment variable name (e.g., `env:NODE_TOKEN_1`).
2. **File References**: Prefix `file:`, followed by the absolute path to a file
   containing the secret/token. The path must live under the node-secrets root
   (default `/etc/gah/node-secrets`, override with `GAH_NODE_SECRETS_ROOT`) --
   e.g. `file:/etc/gah/node-secrets/node-1.token`. This restriction exists
   because health checks fetch an operator-supplied `advertised_url`; without
   it, a registrant could point that URL at a server they control and use the
   coordinator to read (and exfiltrate) any file readable by the server
   process.

### Triggering Secret Rotation:
To rotate the secret reference of a registered node, use the `POST /api/registry/nodes/:nodeId/rotate-secret` endpoint.

#### API Request Example:
```bash
curl -X POST https://coordinator.lan/api/registry/nodes/node-1/rotate-secret \
  -H "Authorization: Bearer <COORDINATOR_TOKEN>" \
  -H "Content-Type: application/json" \
  -d '{
    "secret_ref": "env:NODE_TOKEN_NEW"
  }'
```

The coordinator will validate the format of the new `secret_ref`. On success, it will save it to the registry config file and use it for subsequent health checks and node requests.

## 2. Node Revocation

To revoke node access, a coordinator administrator can completely delete a node registration. This will prevent the coordinator from querying the node and reject any requests originating from that node if it tries to connect to the fleet.

### Revoking a Node registration:
Use the `DELETE /api/registry/nodes/:nodeId` endpoint.

#### API Request Example:
```bash
curl -X DELETE https://coordinator.lan/api/registry/nodes/node-1 \
  -H "Authorization: Bearer <COORDINATOR_TOKEN>"
```

On success, the registry config will be updated, and the node metadata is purged from the registry, immediately halting all outbound transport/health checks to that node.
