/**
 * Shared utilities for Git Agent Harness
 */
import { ProviderKind, SessionId, ProviderInstanceId } from "@git-agent-harness/contracts";
export declare function generateSessionId(): SessionId;
export declare function generateRequestId(): string;
export declare function generateProviderInstanceId(kind: ProviderKind, index?: number): ProviderInstanceId;
export declare function formatTimestamp(timestamp: number): string;
export declare function parseJsonConfig(content: string): Record<string, unknown>;
export declare function getEnvVar(name: string, defaultValue?: string): string;
export declare function isProviderAvailable(kind: ProviderKind): boolean;
export declare function getSupportedProviders(): ProviderKind[];
export declare function getSessionStatusColor(status: string): string;
export declare class GAHError extends Error {
    readonly code: string;
    readonly details?: unknown | undefined;
    constructor(message: string, code: string, details?: unknown | undefined);
}
export declare function createErrorResponse(requestId: string, error: Error): {
    type: string;
    error: string;
    requestId: string;
};
