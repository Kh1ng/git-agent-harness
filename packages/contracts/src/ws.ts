/**
 * WebSocket contract types for Git Agent Harness
 * Inspired by t3code architecture but adapted for GAH needs
 */

import type { MergeRequest, AvailabilityScope, Blocker, StatusError, RecentLedgerSummary, DependencyBlocker } from './gah.js';
import type { ChatSessionSummary, ChatSessionView, ChatTranscriptTurn } from './chat-session.js';

// Provider types
export type ProviderKind = 
  | "github" 
  | "gitlab" 
  | "codex" 
  | "claude" 
  | "cursor" 
  | "opencode" 
  | "grok" 
  | "openhands"
  | "agy"
  | "vibe"
  | "hermes"
  | "auto";

export type ProviderInstanceId = string;

export type ProviderStatus = 
  | { type: "unavailable"; reason?: string }
  | { type: "available"; version: string }
  | { type: "authenticated"; version: string; userId: string }
  | { type: "error"; error: string }
  | { type: "not_implemented" };

// Session types
export type SessionId = string;
export type SessionStatus = "idle" | "starting" | "running" | "stopping" | "stopped" | "error";
export type DispatchLeaseState =
  | "offered"
  | "accepted"
  | "running"
  | "terminal"
  | "expired"
  | "uncertain_reconciling";

// Session type - manually defined instead of using Effect Schema
// to avoid version compatibility issues
export interface Session {
  id: SessionId;
  providerKind: ProviderKind;
  instanceId: ProviderInstanceId;
  status: SessionStatus;
  /** Node that currently owns the dispatch/session, when routed through a fleet coordinator. */
  nodeId?: string;
  /** Distributed lease state for fleet-coordinated dispatches. */
  leaseState?: DispatchLeaseState;
  startedAt?: string;
  endedAt?: string;
  error?: string;
  repo?: string;
  branch?: string;
  target?: string;
  mode?: string;
  backend?: string;
  model?: string;
  budget?: number;
}

// WebSocket message types
export type ServerMessage = 
  | {
      type: "server.welcome";
      serverVersion: string;
      serverProviderCatalog: ServerProviderCatalog;
      sessions: Session[];
      providers: Record<ProviderInstanceId, ProviderStatus>;
      // TICKET-114: Real GAH data from CLI
      profile?: string;
      mergeRequests?: MergeRequest[];
      availability?: AvailabilityScope[];
      blockers?: Blocker[];
      constraints?: Blocker[];
      errors?: StatusError[];
      recentLedger?: RecentLedgerSummary | null;
      // Native issue prerequisites that block autonomous intake.
      dependencyBlockers?: DependencyBlocker[];
      // TICKET-157: per-backend "configured for this profile" signal,
      // derived from the Rust harness `configured_backend_path()`.
      // Maps a backend name (e.g. "codex", "opencode") to whether it has
      // a real implementation and is wired for the active profile.
      backendConfigured?: Record<string, boolean>;
    }
  | {
      type: "server.ping";
      timestamp: number;
    }
  | {
      type: "session.started";
      session: Session;
    }
  | {
      type: "session.stopped";
      session: Session;
    }
  | {
      type: "session.status";
      session: Session;
    }
  | {
      type: "session.stdout";
      sessionId: SessionId;
      data: string;
      timestamp: number;
    }
  | {
      type: "session.stderr";
      sessionId: SessionId;
      data: string;
      timestamp: number;
    }
  | {
      type: "provider.statusChanged";
      instanceId: ProviderInstanceId;
      status: ProviderStatus;
    }
  | {
      type: "provider.listUpdated";
      providers: Record<ProviderInstanceId, ProviderStatus>;
    }
  | {
      type: "error";
      error: string;
      requestId: string;
    }
  | {
      /** Live structured tool-call activity (slice 3): the ACP tool_call
       * stream as typed events, so clients render what the agent is doing
       * while the turn is in flight. Mirrors the session log's tool/call
       * events. */
      type: "manager.chat.toolCall";
      requestId: string;
      profile: string;
      sessionId?: string;
      turn: number;
      toolCallId: string;
      name: string | null;
      title: string;
      kind: string | null;
      status: "pending" | "completed" | "failed";
      locations: string[];
      summary: string | null;
    }
  | {
      /** A backend permission request awaiting a human decision (slice 3).
       * The turn is blocked until manager.chat.permission.respond arrives,
       * the turn is cancelled, or the timeout cancels fail-closed. */
      type: "manager.chat.permission";
      requestId: string;
      profile: string;
      sessionId?: string;
      turn: number;
      permissionId: string;
      title: string;
      options: { optionId: string; name: string; kind: string }[];
      locations: string[];
    }
  | {
      /** WP3: a session's preview went live (manual set or auto-detected
       * from dev-server output in tool events). Clients light up the
       * Preview affordance. */
      type: "manager.chat.preview";
      requestId: string;
      profile: string;
      sessionId: string;
      devPort: number;
      listenPort: number;
      url: string;
    }
  | {
      /** A steering message was injected into the currently running turn. */
      type: "manager.chat.steered";
      requestId: string;
      profile: string;
      sessionId?: string;
      outcome: "injected";
    }
  | {
      type: "manager.chat.reply";
      requestId: string;
      profile: string;
      sessionId?: string;
      reply: string;
      backend: string;
      model: string | null;
      usage: ChatTranscriptTurn['usage'];
      /** True when the turn was stopped (Stop button) rather than completed;
       * the reply is the partial text captured before the stop. Clients
       * clear their in-flight busy state either way. */
      cancelled?: boolean;
    }
  | {
      /** Live tee of the session log's assistant/chunk events for one turn
       * (#959). The session log is the record; this is a live push of the
       * same writes so every client subscribed to the profile renders the
       * turn progressively. `seq` matches the logged assistant/chunk event. */
      type: "manager.chat.chunk";
      requestId: string;
      profile: string;
      sessionId?: string;
      turn: number;
      seq: number;
      text: string;
    }
  | {
      type: "manager.chat.history";
      requestId: string;
      profile: string;
      sessionId?: string;
      turns: ManagerChatTurn[];
      /** Monotonic log position; the client can resume requesting after it. */
      cursor: number;
      /** Partial assistant reply when the requested turn is still running. */
      streaming: ChatSessionView['streaming'];
      /** Actionable permission still blocking the live turn, if any. */
      permission: ChatSessionView['permission'];
    }
  | {
      /** Signals clients to reload one profile after its active turn ends. */
      type: "manager.chat.updated";
      profile: string;
      sessionId?: string;
      requestId: string;
    }
  | {
      type: "manager.chat.sessionList";
      requestId: string;
      profile: string;
      sessions: ChatSessionSummary[];
    }
  | {
      type: "manager.chat.sessionCreated";
      requestId: string;
      profile: string;
      session: ChatSessionSummary;
    }
  | {
      type: "manager.chat.sessionUpdated";
      requestId: string;
      profile: string;
      session: ChatSessionSummary;
    }
  | {
      type: "manager.chat.sessionArchived";
      requestId: string;
      profile: string;
      session: ChatSessionSummary;
    };

export type ManagerChatTurn = ChatTranscriptTurn;

export type ClientMessage = 
  | {
      type: "client.hello";
      clientVersion: string;
      profile?: string;
      capabilities: ClientCapabilities;
    }
  | {
      type: "session.start";
      requestId: string;
      /** Optional explicit node pin for fleet routing. */
      nodeId?: string;
      /** Internal routing context used when the coordinator forwards a request to a worker. */
      coordinatorNodeId?: string;
      // GAH profile id (config.toml's [profiles.<id>], e.g. "gah",
      // "worldcup-props") -- NOT a backend name like "codex"/"claude".
      // `gah dispatch --profile <profile>` needs this exact value.
      profile: string;
      providerKind: ProviderKind;
      instanceId: ProviderInstanceId;
      repo: string;
      branch?: string;
      target?: string;
      mode: string;
      backend?: string;
      model?: string;
      budget?: number;
    }
  | {
      type: "session.stop";
      requestId: string;
      sessionId: SessionId;
    }
  | {
      type: "session.sendCommand";
      requestId: string;
      sessionId: SessionId;
      command: string;
    }
  | {
      type: "provider.refresh";
      requestId: string;
      instanceId: ProviderInstanceId;
    }
  | {
      type: "provider.list";
      requestId: string;
    }
  | {
      type: "ping";
      requestId: string;
      timestamp: number;
    }
  | {
      type: "manager.chat.send";
      requestId: string;
      profile: string;
      message: string;
      /** WP2 sessions: target session within the profile. Omitted (or
       * 'default') addresses the profile's legacy single conversation. */
      sessionId?: string;
    }
  | {
      /** Inject a user follow-up into the active turn on the same backend
       * session. This is distinct from queueing a later turn. */
      type: "manager.chat.steer";
      requestId: string;
      profile: string;
      message: string;
      sessionId?: string;
    }
  | {
      /** Stops the in-flight turn for a profile (#960). Safe to send when no
       * turn is running; the server treats it as a no-op then. */
      type: "manager.chat.cancel";
      requestId: string;
      profile: string;
      sessionId?: string;
    }
  | {
      type: "manager.chat.historyRequest";
      requestId: string;
      profile: string;
      sessionId?: string;
    }
  | {
      /** Answer a live permission request (slice 3). */
      type: "manager.chat.permission.respond";
      requestId: string;
      profile: string;
      sessionId?: string;
      permissionId: string;
      optionId: string;
    }
  | {
      /** WP2: list a profile's chat sessions. */
      type: "manager.chat.sessionList";
      requestId: string;
      profile: string;
    }
  | {
      /** WP2: create a session bound to a fresh worktree. */
      type: "manager.chat.sessionCreate";
      requestId: string;
      profile: string;
      /** Backend to serve the session; omitted = profile default. */
      backend?: string;
      /** Model override for the session's backend; omitted = backend default. */
      model?: string;
      /** Per-session reasoning effort; omitted = backend default. */
      reasoningEffort?: string;
      title?: string;
    }
  | {
      /** WP2: change a live session's backend and/or model. The worktree
       * stays; the next turn runs on the new backend/model in the same
       * directory -- the manual form of backend interchange. */
      type: "manager.chat.sessionUpdate";
      requestId: string;
      profile: string;
      sessionId: string;
      backend?: string;
      model?: string | null;
      reasoningEffort?: string | null;
      title?: string;
    }
  | {
      /** WP2: archive a session -- dirty worktree is patched into the
       * session's state dir first, then the worktree is removed. The branch
       * and the event log survive for later resume. */
      type: "manager.chat.sessionArchive";
      requestId: string;
      profile: string;
      sessionId: string;
    };

export type ClientCapabilities = {
  supportsTerminal: boolean;
  supportsNotifications: boolean;
  version: string;
};

// Server provider catalog
export type ServerProviderCatalog = {
  providers: ProviderInstance[];
};

export type ProviderInstance = {
  instanceId: ProviderInstanceId;
  providerKind: ProviderKind;
  name: string;
  isAvailable: boolean;
  isAuthenticated: boolean;
  version: string;
};

// Rust backend integration types
export type RustBackendRequest = {
  type: "rust.dispatch" | "rust.status" | "rust.ledger" | "rust.sync" | "rust.availability";
  payload: unknown;
};

export type RustBackendResponse = {
  type: string;
  success: boolean;
  data?: unknown;
  error?: string;
};

// Provider configuration types (mirroring Rust config)
export type ProfileConfig = {
  display_name: string;
  repo_id: string;
  provider: ProviderKind;
  repo: string;
  local_path: string;
  artifact_root: string;
  default_target_branch: string;
  provider_api_base?: string;
  provider_project_id?: string;
  oh_profile?: string;
  model_improve?: string;
  model_pm?: string;
  model_review?: string;
  validation_commands: string[];
  test_file_patterns: string[];
};

export type RoutingPolicy = {
  default_backend?: string;
  review_backend?: string;
  weak_review_backend?: string;
  pm_backend?: string;
  improve_backend?: string;
  allow_review_fallback?: boolean;
};

export type DefaultsConfig = {
  artifact_root: string;
  worktree_base: string;
  llm_base_url: string;
  llm_model_local: string;
  llm_model_cloud: string;
  routing: RoutingPolicy;
};

export type GAHConfig = {
  defaults: DefaultsConfig;
  profiles: Record<string, ProfileConfig>;
};

// Server provider types
export type ServerProvider = {
  kind: ProviderKind;
  version: string;
  status: ProviderStatus;
  capabilities: Record<string, boolean>;
  metadata?: Record<string, unknown>;
};
