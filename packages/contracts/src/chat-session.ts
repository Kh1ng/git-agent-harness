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
  | ChatToolResult
  | ChatHarnessError
  | ChatHumanCommand
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

export type ChatUserMessageSource = 'prompt' | 'inject';

/** A user-role message: a human prompt or injected context. */
export interface ChatUserMessage {
  type: 'user/message';
  seq: number;
  turn: number;
  text: string;
  source: ChatUserMessageSource;
  timestamp: number;
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
  role: 'user' | 'assistant' | 'system';
  text: string;
  timestamp: number;
  /** Present on assistant turns: which backend + model produced this reply. */
  backend?: string;
  model?: string | null;
  usage?: ChatUsage | null;
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
}
