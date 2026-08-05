/**
 * Shared utilities for Git Agent Harness
 */

import { ProviderKind, SessionId, ProviderInstanceId } from "@git-agent-harness/contracts";

// crypto.randomUUID() only exists in a secure context (https:// or
// localhost) -- browsers strip it on plain http:// LAN access. But
// crypto.getRandomValues() has no such restriction, so prefer it over a
// predictable Math.random()-based fallback (flagged by CodeQL as
// js/insecure-randomness when it feeds a session ID -- correctly, since a
// PRNG-based session ID is a real weakness even if this codebase doesn't
// currently use these IDs for auth).
function uuidV4(): string {
  if (typeof crypto !== "undefined" && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  if (typeof crypto !== "undefined" && crypto.getRandomValues) {
    const bytes = crypto.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
    const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  }
  // Unreachable in any real browser or Node runtime (both have had
  // getRandomValues for years) -- last-resort only.
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
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