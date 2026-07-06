/**
 * Shared utilities for Git Agent Harness
 */
// Generate unique IDs
export function generateSessionId() {
    return `session_${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;
}
export function generateRequestId() {
    return `req_${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;
}
export function generateProviderInstanceId(kind, index = 0) {
    return `${kind}_instance_${index}`;
}
// Format timestamps
export function formatTimestamp(timestamp) {
    return new Date(timestamp).toISOString();
}
// Parse configuration files
export function parseJsonConfig(content) {
    try {
        return JSON.parse(content);
    }
    catch (error) {
        throw new Error(`Failed to parse JSON config: ${error instanceof Error ? error.message : String(error)}`);
    }
}
// Environment utilities
export function getEnvVar(name, defaultValue = '') {
    return process.env[name] ?? defaultValue;
}
// Provider utilities
export function isProviderAvailable(kind) {
    const availableProviders = [
        "github",
        "gitlab",
        "codex",
        "claude",
        "cursor",
        "opencode",
        "grok",
        "openhands",
        "agy",
        "vibe"
    ];
    return availableProviders.includes(kind);
}
export function getSupportedProviders() {
    return [
        "github",
        "gitlab",
        "codex",
        "claude",
        "cursor",
        "opencode",
        "grok",
        "openhands",
        "agy",
        "vibe",
        "auto"
    ];
}
// Session utilities
export function getSessionStatusColor(status) {
    const colors = {
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
    code;
    details;
    constructor(message, code, details) {
        super(message);
        this.code = code;
        this.details = details;
        this.name = 'GAHError';
    }
}
export function createErrorResponse(requestId, error) {
    return {
        type: 'error',
        error: error instanceof Error ? error.message : String(error),
        requestId
    };
}
