/**
 * Provider Driver Interface
 * Inspired by t3code's ProviderDriver interface
 */

import type { ProviderKind, ServerProvider, Session } from '@git-agent-harness/contracts';

// Base environment requirements that all drivers might need
export type ProviderDriverEnv = {
  // Common environment variables
  NODE_ENV?: string;
  HOME?: string;
  USER?: string;
  PATH?: string;
};

/**
 * ProviderDriver interface - defines the contract that all provider drivers must implement
 */
export interface ProviderDriver<R extends ProviderDriverEnv = ProviderDriverEnv> {
  /** Unique identifier for this driver type */
  readonly kind: ProviderKind;
  
  /** Version of this driver implementation */
  readonly version: string;
  
  /**
   * Create a new instance of the driver with the given environment
   */
  create: (env: R) => ProviderDriverInstance<R>;
  
  /**
   * Create a snapshot of the current provider state (for quick status checks)
   */
  createSnapshot: () => Promise<ServerProvider>;
}

/**
 * ProviderDriverInstance - the actual driver instance interface
 */
export interface ProviderDriverInstance<R extends ProviderDriverEnv = ProviderDriverEnv> {
  /** Unique identifier for this driver type */
  readonly kind: ProviderKind;
  
  /** Version of this driver implementation */
  readonly version: string;
  
  /**
   * Get a snapshot of the current provider state
   */
  getSnapshot(): Promise<ServerProvider>;
  
  /**
   * Refresh the provider state and return updated snapshot
   */
  refresh(): Promise<ServerProvider>;
  
  /**
   * Start a new session with this provider
   */
  startSession(options: {
    repo: string;
    branch?: string;
    target?: string;
    mode: string;
    backend?: string;
    model?: string;
    budget?: number;
  }): Promise<Session>;
  
  /**
   * Stop an existing session
   */
  stopSession(sessionId: string): Promise<Session>;
  
  /**
   * Send a command to an existing session
   */
  sendCommand(sessionId: string, command: string): Promise<void>;
}

// Type representing any provider driver
export type AnyProviderDriver<R extends ProviderDriverEnv = ProviderDriverEnv> = ProviderDriver<R>;