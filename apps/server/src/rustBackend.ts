/**
 * GAH backend integration
 * 
 * After TICKET-113: The broken stdin/stdout child-process bridge has been removed.
 * The server now shells out to the real `gah` CLI per action via gahCli.ts.
 * 
 * This file is kept for backward compatibility during the transition, but
 * the primary integration is now through gahCli.ts which provides:
 * - runStatus() -> calls `gah status --profile <p> --json`
 * - runDispatch() -> calls `gah dispatch ...` with streaming output
 * - runEvents() -> calls `gah events --profile <p> --since <ts> --json`
 * 
 * The old RustBackendProxy class and its sendCommand() method are deprecated.
 * All real work should use gahCli.ts instead.
 */

import { markReadinessCheck } from './serverReadiness.js';

/**
 * Deprecated: RustBackendProxy is no longer used for actual work.
 * Kept for backward compatibility during transition.
 * 
 * @deprecated Use gahCli.ts (runStatus, runDispatch, runEvents) instead
 */
class RustBackendProxy {
  private isReady = false;
  
  constructor() {
    // The old process-based approach has been removed.
    // All CLI integration now goes through gahCli.ts
  }
  
  async start(): Promise<boolean> {
    // Mark as ready - the real integration uses gahCli.ts which
    // shells out to the CLI per action
    this.isReady = true;
    markReadinessCheck('rustBackend', true, 'Using gah CLI via gahCli.ts');
    return true;
  }
  
  async stop(): Promise<void> {
    this.isReady = false;
    markReadinessCheck('rustBackend', false, 'Stopped');
  }
  
  /**
   * @deprecated This method no longer works. Use gahCli.ts functions instead.
   * @throws Error Always throws - use gahCli.runDispatch() instead
   */
  async sendCommand(_command: string, _args: string[] = []): Promise<string> {
    throw new Error(
      'sendCommand() is deprecated. Use gahCli.ts (runStatus, runDispatch, runEvents) instead. '
      + 'See TICKET-113 for details.'
    );
  }
  
  isBackendReady(): boolean {
    return this.isReady;
  }
}

const rustBackend = new RustBackendProxy();

export async function startRustBackendProxy(): Promise<void> {
  try {
    const success = await rustBackend.start();
    if (success) {
      console.log('GAH backend integration ready (using gahCli.ts)');
    } else {
      console.warn('GAH backend integration failed');
    }
  } catch (error) {
    console.warn('Failed to initialize GAH backend:', error);
    markReadinessCheck('rustBackend', true, 'Running in limited mode');
  }
}

export async function stopRustBackendProxy(): Promise<void> {
  await rustBackend.stop();
}

export function getRustBackendProxy(): RustBackendProxy {
  return rustBackend;
}

export { RustBackendProxy };

// Re-export the new CLI integration
import * as gahCli from './gahCli.js';
export { gahCli };
export const { runStatus, runDispatch, runEvents, getGahBinaryPath } = gahCli;