/**
 * WebSocket contract types for Git Agent Harness
 * Inspired by t3code architecture but adapted for GAH needs
 */
import * as S from "effect/Schema";
export type ProviderKind = "github" | "gitlab" | "codex" | "claude" | "cursor" | "opencode" | "grok" | "openhands" | "agy" | "vibe" | "auto";
export type ProviderInstanceId = string;
export type ProviderStatus = {
    type: "unavailable";
} | {
    type: "available";
    version: string;
} | {
    type: "authenticated";
    version: string;
    userId: string;
} | {
    type: "error";
    error: string;
};
export type SessionId = string;
export type SessionStatus = "idle" | "starting" | "running" | "stopping" | "stopped" | "error";
export declare const SessionSchema: S.Struct<{
    id: typeof S.String;
    providerKind: S.Literal<["github", "gitlab", "codex", "claude", "cursor", "opencode", "grok", "openhands", "agy", "vibe", "auto"]>;
    instanceId: typeof S.String;
    status: S.Literal<["idle", "starting", "running", "stopping", "stopped", "error"]>;
    startedAt: S.optional<typeof S.String>;
    endedAt: S.optional<typeof S.String>;
    error: S.optional<typeof S.String>;
    repo: S.optional<typeof S.String>;
    branch: S.optional<typeof S.String>;
    target: S.optional<typeof S.String>;
    mode: S.optional<typeof S.String>;
    backend: S.optional<typeof S.String>;
    model: S.optional<typeof S.String>;
    budget: S.optional<typeof S.Number>;
}>;
export type Session = typeof SessionSchema.Type;
export type ServerMessage = {
    type: "server.welcome";
    serverVersion: string;
    serverProviderCatalog: ServerProviderCatalog;
    sessions: Session[];
    providers: Record<ProviderInstanceId, ProviderStatus>;
} | {
    type: "server.ping";
    timestamp: number;
} | {
    type: "session.started";
    session: Session;
} | {
    type: "session.stopped";
    session: Session;
} | {
    type: "session.status";
    session: Session;
} | {
    type: "session.stdout";
    sessionId: SessionId;
    data: string;
    timestamp: number;
} | {
    type: "session.stderr";
    sessionId: SessionId;
    data: string;
    timestamp: number;
} | {
    type: "provider.statusChanged";
    instanceId: ProviderInstanceId;
    status: ProviderStatus;
} | {
    type: "provider.listUpdated";
    providers: Record<ProviderInstanceId, ProviderStatus>;
} | {
    type: "error";
    error: string;
    requestId: string;
};
export type ClientMessage = {
    type: "client.hello";
    clientVersion: string;
    capabilities: ClientCapabilities;
} | {
    type: "session.start";
    requestId: string;
    providerKind: ProviderKind;
    instanceId: ProviderInstanceId;
    repo: string;
    branch?: string;
    target?: string;
    mode: string;
    backend?: string;
    model?: string;
    budget?: number;
} | {
    type: "session.stop";
    requestId: string;
    sessionId: SessionId;
} | {
    type: "session.sendCommand";
    requestId: string;
    sessionId: SessionId;
    command: string;
} | {
    type: "provider.refresh";
    requestId: string;
    instanceId: ProviderInstanceId;
} | {
    type: "provider.list";
    requestId: string;
} | {
    type: "ping";
    requestId: string;
    timestamp: number;
};
export type ClientCapabilities = {
    supportsTerminal: boolean;
    supportsNotifications: boolean;
    version: string;
};
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
export type ServerProvider = {
    kind: ProviderKind;
    version: string;
    status: ProviderStatus;
    capabilities: Record<string, boolean>;
    metadata?: Record<string, unknown>;
};
