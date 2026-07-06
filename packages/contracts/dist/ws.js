/**
 * WebSocket contract types for Git Agent Harness
 * Inspired by t3code architecture but adapted for GAH needs
 */
import * as S from "effect/Schema";
export const SessionSchema = S.Struct({
    id: S.String,
    providerKind: S.Literal(...["github", "gitlab", "codex", "claude", "cursor", "opencode", "grok", "openhands", "agy", "vibe", "auto"]),
    instanceId: S.String,
    status: S.Literal(...["idle", "starting", "running", "stopping", "stopped", "error"]),
    startedAt: S.optional(S.String),
    endedAt: S.optional(S.String),
    error: S.optional(S.String),
    repo: S.optional(S.String),
    branch: S.optional(S.String),
    target: S.optional(S.String),
    mode: S.optional(S.String),
    backend: S.optional(S.String),
    model: S.optional(S.String),
    budget: S.optional(S.Number),
});
