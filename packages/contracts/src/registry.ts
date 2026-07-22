export interface RegisteredNode {
  node_id: string;
  display_name: string;
  advertised_url: string;
  version: string;
  schema_digest: string;
  labels?: string[];
  transport_mode: 'loopback' | 'authenticated_remote' | 'trusted_lan';
  secret_ref: string; // reference like "env:NODE_TOKEN" or "file:/path/to/token.txt"
}

export interface NodeSummary {
  node_id: string;
  display_name: string;
  advertised_url: string;
  version: string;
  schema_digest: string;
  labels?: string[];
  transport_mode: 'loopback' | 'authenticated_remote' | 'trusted_lan';
}

export type HealthCheckFailureKind =
  | 'DNS'
  | 'NETWORK'
  | 'TLS'
  | 'AUTH'
  | 'PROTOCOL'
  | 'VERSION'
  | 'SCHEMA';

export interface NodeHealthCheckResult {
  node_id: string;
  status: 'healthy' | 'unhealthy';
  timestamp: number;
  error?: {
    kind: HealthCheckFailureKind;
    message: string;
  };
}

export interface RegistryConfig {
  nodes: RegisteredNode[];
}
