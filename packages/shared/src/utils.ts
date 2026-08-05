/**
 * Shared utilities for Git Agent Harness
 */

import { ProviderKind, SessionId, ProviderInstanceId } from "@git-agent-harness/contracts";

// crypto.randomUUID() only exists in a secure context (https:// or
// localhost) -- browsers strip it on plain http:// LAN access. But
// crypto.getRandomValues() has no such restriction, so use it instead of
// falling back to Math.random(). No Math.random() fallback below that:
// CodeQL's dataflow analysis (js/insecure-randomness) flags any static path
// from Math.random() into a session ID regardless of runtime guards around
// it, and a silent downgrade to predictable randomness is the wrong
// behavior anyway -- getRandomValues has been in every real browser and
// Node runtime for years, so if it's somehow missing, throwing here is
// correct: better than minting a weak ID no caller expects.
function uuidV4(): string {
  if (typeof crypto !== "undefined" && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

// Generate unique IDs
export function generateSessionId(): SessionId {
  return `session_${Date.now()}_${uuidV4()}`;
}

export function generateRequestId(): string {
  return `req_${Date.now()}_${uuidV4()}`;
}

export function generateProviderInstanceId(kind: ProviderKind, index: number = 0): ProviderInstanceId {
  return `${kind}_instance_${index}`;
}

// Format timestamps
export function formatTimestamp(timestamp: number): string {
  return new Date(timestamp).toISOString();
}

// Parse configuration files
export function parseJsonConfig(content: string): Record<string, unknown> {
  try {
    return JSON.parse(content);
  } catch (error) {
    throw new Error(`Failed to parse JSON config: ${error instanceof Error ? error.message : String(error)}`);
  }
}

// Environment utilities
export function getEnvVar(name: string, defaultValue: string = ''): string {
  return process.env[name] ?? defaultValue;
}

// Provider utilities
// NOTE(TICKET-157): `grok` and `cursor` exist only as UI scaffolding in
// `ProviderKind`/`getSupportedProviders` -- they have zero backend
// implementation in the Rust harness (no config field, no dispatch match
// arm, no path/args override). They are intentionally excluded here so
// Settings does not show a bogus "available" status for something that
// cannot run. Use the `not_implemented` ProviderStatus variant to surface
// them as UI placeholders if needed.
export function isProviderAvailable(kind: ProviderKind): boolean {
  const availableProviders: ProviderKind[] = [
    "github",
    "gitlab", 
    "codex",
    "claude",
    "opencode",
    "openhands",
    "agy",
    "vibe"
  ];
  return availableProviders.includes(kind);
}

export function getSupportedProviders(): ProviderKind[] {
  return [
    "github",
    "gitlab",
    "codex", 
    "claude",
    "opencode",
    "openhands",
    "agy",
    "vibe",
    "auto"
  ];
}

// Session utilities
export function getSessionStatusColor(status: string): string {
  const colors: Record<string, string> = {
    idle: '#6b7280',
    starting: '#f59e0b',
    running: '#10b981',
    stopping: '#f59e0b',
    stopped: '#6b7280',
    error: '#ef4444'
  };
  return colors[status] || '#6b7280';
}

// Error handling
export class GAHError extends Error {
  constructor(
    message: string,
    public readonly code: string,
    public readonly details?: unknown
  ) {
    super(message);
    this.name = 'GAHError';
  }
}

export function createErrorResponse(requestId: string, error: Error): { type: string; error: string; requestId: string } {
  return {
    type: 'error',
    error: error instanceof Error ? error.message : String(error),
    requestId
  };
}