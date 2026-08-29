/**
 * Event-sourced chat session log (issue #955): the t3-style control-surface
 * chat spine.
 *
 * A project chat is an append-only log of typed session events. The message
 * transcript the UI renders is DERIVED from the log (a fold), never stored
 * separately -- so reload, resume, per-turn attribution, and compaction all
 * derive from the same stream. This mirrors the session architecture in
 * deepseek-harness (dsh): "model-visible means logged", and replay is
 * re-derivation.
 *
 * Every event is lossless JSON and carries a monotonic `seq`. An interrupted
 * turn (turn/start with no matching turn/end) is closed with a synthetic
 * `turn/end { reason: 'interrupted' }` on reload -- never truncated.
 */

export type ChatSessionEvent =
  | ChatTurnStart
  | ChatTurnEnd
  | ChatUserMessage
  | ChatAssistantChunk
  | ChatAssistantMessage
  | ChatToolCallEvent
  | ChatPermissionRequest
  | ChatPermissionDecision
  | ChatToolResult
  | ChatHarnessError
  | ChatHumanCommand
  | ChatHandoff
  | ChatCompactionStart
  | ChatCompactionSummary
  | ChatCompactionEnd;

/** One model exchange (or a zero-step turn that spent no model call). */
export interface ChatTurnStart {
  type: 'turn/start';
  seq: number;
  turn: number;
  timestamp: number;
}

export type ChatTurnEndReason =
  | { kind: 'complete' }
  | { kind: 'error'; message?: string }
  | { kind: 'cancelled' }
  /** Synthetic, written only on reload repair for a turn left open by a crash. */
  | { kind: 'interrupted' };

export interface ChatTurnEnd {
  type: 'turn/end';
  seq: number;
  turn: number;
  reason: ChatTurnEndReason;
  timestamp: number;
}

export type ChatUserMessageSource = 'prompt' | 'inject' | 'steer';

/** A user-role message: a turn prompt, injected context, or mid-turn steer. */
export interface ChatUserMessage {
  type: 'user/message';
  seq: number;
  turn: number;
  text: string;
  source: ChatUserMessageSource;
  timestamp: number;
  /** #961: provenance for injected context -- the policy and budget in force
   * when this context was injected, plus whether the budget truncated it.
   * Only present on source: 'inject' events. */
  policy?: { budgetChars?: number; tiers?: string[] };
  truncated?: boolean;
}

/** Raw assistant stream chunk -- token-level replay/typing fidelity. */
export interface ChatAssistantChunk {
  type: 'assistant/chunk';
  seq: number;
  turn: number;
  text: string;
  timestamp: number;
}

/**
 * The assembled assistant message for one step. Carries per-message
 * attribution (which backend + model produced it) plus usage when the
 * backend reported token accounting.
 */
export interface ChatAssistantMessage {
  type: 'assistant/message';
  seq: number;
  turn: number;
  text: string;
  backend: string;
  model: string | null;
  usage: ChatUsage | null;
  timestamp: number;
}

export interface ChatUsage {
  input_tokens: number | null;
  output_tokens: number | null;
  total_tokens: number | null;
  estimated_cost_usd: number | null;
  duration_seconds: number | null;
}

/** A backend tool result surfaced in chat (e.g. /command output). */
export interface ChatToolResult {
  type: 'tool/result';
  seq: number;
  turn: number;
  name: string;
  text: string;
  timestamp: number;
}

/**
 * A structured tool-call activity event (slice 3): the ACP tool_call stream
 * surfaced as first-class session events instead of flattened text, so the
 * UI renders what the agent is doing (with file locations) as it happens.
 * `pending` events are live (in-flight tool calls); `completed`/`failed`
 * carry a short output summary. Persisted in the session log, replayed on
 * resume, and pushed live over the WS.
 */
export interface ChatToolCallEvent {
  type: 'tool/call';
  seq: number;
  turn: number;
  /** Stable id within the backend session (ACP toolCallId). */
  toolCallId: string;
  /** Programmatic tool name, when the backend provides one. */
  name: string | null;
  /** Human-readable title ("Reading src/main.rs"). */
  title: string;
  kind: string | null;
  status: 'pending' | 'completed' | 'failed';
  /** Absolute paths the tool touched, for follow-along UI. */
  locations: string[];
  /** Short output summary once finished (content/rawOutput digest). */
  summary: string | null;
  timestamp: number;
}

/** A permission request from the backend, awaiting a human decision
 * (slice 3). One is live at a time per conversation; the turn blocks until
 * the client answers (manager.chat.permission.respond), the turn is
 * cancelled, or the timeout elapses (fail-closed: cancel). */
export interface ChatPermissionRequest {
  type: 'permission/request';
  seq: number;
  turn: number;
  /** Opaque id the client echoes back in the response. */
  permissionId: string;
  /** What the backend wants to do ("Run `cargo test`"). */
  title: string;
  /** The selectable options, in the backend's own order. */
  options: { optionId: string; name: string; kind: string }[];
  /** File locations involved, when known. */
  locations: string[];
  timestamp: number;
}

/** The recorded decision for a permission request. */
export interface ChatPermissionDecision {
  type: 'permission/decision';
  seq: number;
  turn: number;
  permissionId: string;
  /** The chosen optionId, or 'cancelled' when nobody answered in time. */
  optionId: string;
  timestamp: number;
}

/** An infrastructure failure shown in chat but never replayed to a model. */
export interface ChatHarnessError {
  type: 'harness/error';
  seq: number;
  turn: number;
  text: string;
  timestamp: number;
}

/** A human slash-command result (e.g. /clear, /compact). */
export interface ChatHumanCommand {
  type: 'human/command';
  seq: number;
  turn: number;
  command: string;
  result: string;
  timestamp: number;
}

/** An automatic mid-turn handoff to a fallback backend after a usage/quota
 * limit (#962). Recorded so the transcript explains why the answering model
 * changed -- the per-message backend/model badge then shows the switch. */
export interface ChatHandoff {
  type: 'handoff';
  seq: number;
  turn: number;
  from: string;
  fromModel: string | null;
  to: string;
  toModel: string | null;
  reason: string;
  timestamp: number;
}

/** Delimits one backend context-compaction operation. */
export interface ChatCompactionStart {
  type: 'compaction/start';
  seq: number;
  turn: number;
  timestamp: number;
}

/** Recorded when context is compacted; the transcript carries the summary. */
export interface ChatCompactionSummary {
  type: 'compaction/summary';
  seq: number;
  turn: number;
  summary: string;
  timestamp: number;
}

export interface ChatCompactionEnd {
  type: 'compaction/end';
  seq: number;
  turn: number;
  timestamp: number;
}

/** The derived transcript: message turns with per-message attribution. */
export interface ChatTranscriptTurn {
  role: 'user' | 'assistant' | 'system' | 'tool';
  text: string;
  timestamp: number;
  /** Present on assistant turns: which backend + model produced this reply. */
  backend?: string;
  model?: string | null;
  usage?: ChatUsage | null;
  /** Present on tool turns (slice 3): structured tool-call info for cards. */
  tool?: {
    toolCallId: string;
    name: string | null;
    title: string;
    kind: string | null;
    status: 'pending' | 'completed' | 'failed';
    locations: string[];
    summary: string | null;
  };
}

/** The folded view the chat UI renders from the log. */
export interface ChatSessionView {
  profile: string;
  /** Monotonic log position -- the client can resume requesting after it. */
  cursor: number;
  turns: ChatTranscriptTurn[];
  /** Latest open-turn reason, when a turn is still in progress. */
  streaming?: {
    turn: number;
    partialText: string;
    backend?: string;
    model?: string | null;
  } | null;
  /** Actionable permission still blocking the live turn. Durable so a
   * reconnect can render the same choices and answer by permissionId. */
  permission?: {
    turn: number;
    permissionId: string;
    title: string;
    options: { optionId: string; name: string; kind: string }[];
    locations: string[];
  } | null;
}

/**
 * A durable chat session bound to one worktree (WP2 session service).
 *
 * The session is the portable object: its event log + branch survive backend
 * swaps, server restarts, and worktree reclamation. The worktree itself is a
 * disposable materialization of the branch -- pruned when idle (the
 * `gah-chat-<repo_id>-` prefix makes `gah prune` see it), rematerialized on
 * resume. A session's conversation runs with its worktree as cwd, which is
 * what makes a worktree interchangeable between models.
 */
export interface ChatSessionSummary {
  id: string;
  profile: string;
  /** Absolute worktree path; null once the worktree was reclaimed. */
  worktreePath: string | null;
  /** Branch backing the session (survives worktree reclamation). */
  branch: string;
  /** Backend serving this session (per-session override of the profile default). */
  backend: string;
  /** Model override for the session's backend; null = the backend's default.
   * Applied on the session's connection before each turn. */
  model: string | null;
  title: string | null;
  createdAt: number;
  lastActiveAt: number;
  archivedAt: number | null;
  /** Live sessions are resumable; archived is an operator/idle outcome;
   * settled means provider state proved the work terminal. */
  outcome: 'live' | 'archived' | 'settled';
  settledAt: number | null;
  settledReason: 'merged' | 'closed' | 'delivered' | null;
}

export interface ChatSessionStorage {
  sessionId: string;
  worktreeBytes: number;
  projectedReclaimBytes: number;
  idle: boolean;
}

export interface ChatProfileStorage {
  profile: string;
  idleDays: number;
  worktreeBytes: number;
  projectedReclaimBytes: number;
  sessions: ChatSessionStorage[];
}

export interface ChatReclaimCandidate {
  profile: string;
  sessionId: string;
  outcome: 'archived' | 'settled';
  reason: 'idle' | 'merged' | 'closed' | 'delivered';
  reclaimBytes: number;
}

export interface ChatReclaimResult {
  dryRun: boolean;
  profiles: ChatProfileStorage[];
  candidates: ChatReclaimCandidate[];
  sessions: ChatSessionSummary[];
  warnings: string[];
}

/** A node offered by the new-chat flow's node step. Chat runs on the
 * central node today; workers are listed for visibility but not yet
 * chat-capable (fleet chat is future work). */
export interface ChatNodeInfo {
  nodeId: string;
  displayName: string;
  role: 'central' | 'worker';
  chatCapable: boolean;
  lastSeenAt: string | null;
}

/** WP3: a live session preview — the dedicated port proxying the dev
 * server the agent started inside the session's worktree. */
export interface ChatPreviewInfo {
  profile: string;
  sessionId: string;
  /** Port the dev server listens on inside the node. */
  devPort: number;
  /** Dedicated port the proxy listens on. */
  listenPort: number;
  /** Browser-facing URL (tailscale IP when available). */
  url: string;
}

/** An open provider issue offered by the issue → chat flow. */
export interface ChatIssueSummary {
  number: number;
  title: string;
  url: string | null;
  labels: string[];
  updatedAt: string | null;
}

/** Result of grabbing an issue into a chat session. */
export interface ChatIssueStartResult {
  session: ChatSessionSummary;
  /** True when a live session for this issue already existed and was
   * returned as-is (idempotent grab). */
  existing: boolean;
}

/** An open provider pull request offered by the PR → chat flow. */
export interface ChatPrSummary {
  number: number;
  title: string;
  url: string | null;
  author: string | null;
  headRefName: string | null;
  isDraft: boolean;
  /** Provider review state (e.g. APPROVED, CHANGES_REQUESTED) when reported. */
  reviewState: string | null;
  updatedAt: string | null;
}

/** Result of opening a chat seeded from a pull request. */
export interface ChatPrStartResult {
  session: ChatSessionSummary;
  /** True when a live session for this PR already existed and was
   * returned as-is (idempotent open). */
  existing: boolean;
}
