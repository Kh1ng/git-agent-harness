/**
 * Shared contracts for the CLI Router (CLIProxyAPI) integration.
 *
 * These types define the wire format between GET /api/cli-router,
 * PUT /api/cli-router/settings, and the mutation endpoints.
 * The web UI and server both import from here; never drift.
 */

// ---------------------------------------------------------------------------
// Routing strategy – closed set, no substring inference.
// ---------------------------------------------------------------------------

export type CliRouterStrategy = 'round-robin' | 'fill-first' | 'weighted-round-robin';

export const CLI_ROUTER_STRATEGIES: readonly CliRouterStrategy[] = ['round-robin', 'fill-first', 'weighted-round-robin'] as const;

// ---------------------------------------------------------------------------
// Connection status – discriminated union, fail closed on unknown.
// ---------------------------------------------------------------------------

export type CliRouterStatus = 'unconfigured' | 'connected' | 'unavailable';

// ---------------------------------------------------------------------------
// Per-account quota observation
// ---------------------------------------------------------------------------

export interface CliRouterQuota {
  label: string;
  /** null means the quota value is unknown (not zero). */
  remainingPercent: number | null;
  resetAt: string | null;
  observedAt?: string;
  /** Routing window names use provider keys or reported durations; pools use provider identifiers. */
  window?: string;
  quotaPool?: string;
}

// ---------------------------------------------------------------------------
// Upstream account entry (auth-files projection)
// ---------------------------------------------------------------------------

export interface CliRouterAccount {
  id: string;
  name: string;
  provider: string;
  label: string;
  disabled: boolean;
  unavailable: boolean;
  resetAt: string | null;
  quotas: CliRouterQuota[];
  /** Present only when the provider quota refresh failed for this account. */
  quotaError?: string;
}

// ---------------------------------------------------------------------------
// Model entry (/v1/models projection)
// ---------------------------------------------------------------------------

export interface CliRouterModel {
  id: string;
  ownedBy: string;
}

// ---------------------------------------------------------------------------
// Settings projection (never returns key values)
// ---------------------------------------------------------------------------

export interface CliRouterSettingsView {
  url: string | null;
  hasApiKey: boolean;
  hasManagementKey: boolean;
}

// ---------------------------------------------------------------------------
// Full read-only snapshot returned by GET /api/cli-router
// ---------------------------------------------------------------------------

export interface CliRouterSnapshot {
  settings: CliRouterSettingsView;
  status: CliRouterStatus;
  strategy: CliRouterStrategy;
  sessionAffinity: boolean;
  accounts: CliRouterAccount[];
  models: CliRouterModel[];
}

// ---------------------------------------------------------------------------
// PUT /api/cli-router/settings body
// ---------------------------------------------------------------------------

export interface CliRouterSettingsInput {
  url: string;
  /** Blank/omitted preserves saved value; required on initial connect. */
  apiKey?: string;
  /** Blank/omitted preserves saved value; required on initial connect. */
  managementKey?: string;
  /** Explicit binding of upstream account ids to existing native logical backends. */
  accountBackends?: Record<string, string>;
}

// ---------------------------------------------------------------------------
// POST /api/cli-router/routing body
// ---------------------------------------------------------------------------

export interface CliRouterRoutingInput {
  strategy: CliRouterStrategy;
  sessionAffinity: boolean;
}

// ---------------------------------------------------------------------------
// POST /api/cli-router/accounts/status body
// ---------------------------------------------------------------------------

export interface CliRouterAccountStatusInput {
  id: string;
  disabled: boolean;
}

// ---------------------------------------------------------------------------
// POST /api/cli-router/accounts/refresh body
// ---------------------------------------------------------------------------

export interface CliRouterAccountRefreshInput {
  id: string;
}

// ---------------------------------------------------------------------------
// Mutation operation names – used with mutationSafety
// ---------------------------------------------------------------------------

export type CliRouterOperation =
  | 'cli_router.configure'
  | 'cli_router.routing'
  | 'cli_router.account'
  | 'cli_router.refresh';

// ---------------------------------------------------------------------------
// Stored settings shape (compatible with setup script root writes)
// ---------------------------------------------------------------------------

export interface CliRouterStoredSettings {
  url: string;
  apiKey: string;
  managementKey: string;
  accountBackends?: Record<string, string>;
}

// ---------------------------------------------------------------------------
// Upstream wire types (server-internal but shared for testing)
// ---------------------------------------------------------------------------

/** GET /v0/management/auth-files response shape */
export interface UpstreamAuthFile {
  id: string;
  name: string;
  /** Opaque upstream handle (hex string in v8, number historically). */
  auth_index: string | number;
  provider: string;
  type: string;
  label: string;
  disabled: boolean;
  unavailable: boolean;
  next_retry_after: string | null;
  project_id: string | null;
  quota: unknown;
  model_quotas: unknown;
  /** Safe claims only; present for Codex accounts. */
  id_token?: { chatgpt_account_id?: string } | null;
}

/** GET /v1/models response entry */
export interface UpstreamModel {
  id: string;
  owned_by: string;
}
