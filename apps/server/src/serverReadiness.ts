/**
 * Server readiness tracking for startup barriers
 * Inspired by t3code's ServerReadiness implementation
 */

// Note: Using local contracts package instead of @t3tools/contracts
// import type { ServerReadiness } from '@t3tools/contracts';

type ReadinessCheck = {
  name: string;
  ready: boolean;
  error?: string;
};

class ServerReadinessImpl {
  private _checks: Map<string, ReadinessCheck> = new Map();
  private barriers: Map<string, () => Promise<boolean>> = new Map();
  
  constructor() {
    // Initialize default checks
    this.addCheck('rustBackend', 'Rust backend connection');
    this.addCheck('providerRegistry', 'Provider registry initialization');
    this.addCheck('webSocket', 'WebSocket server initialization');
  }
  
  private addCheck(name: string, description: string) {
    this._checks.set(name, {
      name,
      ready: false,
      error: undefined
    });
  }
  
  addBarrier(name: string, check: () => Promise<boolean>) {
    this.barriers.set(name, check);
  }
  
  async markReady(name: string): Promise<void> {
    const check = this._checks.get(name);
    if (check) {
      check.ready = true;
      check.error = undefined;
    }
    
    // If this was the last barrier, we're ready
    await this.waitForAllBarriers();
  }
  
  markError(name: string, error: string): void {
    const check = this._checks.get(name);
    if (check) {
      check.ready = false;
      check.error = error;
    }
  }
  
  async waitForAllBarriers(): Promise<void> {
    const results = await Promise.allSettled(
      Array.from(this.barriers.entries()).map(([name, check]) => 
        check().then(success => {
          if (success) {
            this.markReady(name);
          } else {
            this.markError(name, `Barrier ${name} failed`);
          }
        })
      )
    );
    
    // Check for errors
    for (const result of results) {
      if (result.status === 'rejected' && result.reason) {
        console.error('Barrier error:', result.reason);
      }
    }
  }
  
  get isReady(): boolean {
    return Array.from(this._checks.values()).every(check => check.ready);
  }
  
  get checks(): Record<string, ReadinessCheck> {
    return Object.fromEntries(this._checks);
  }
  
  getCheck(name: string): ReadinessCheck | undefined {
    return this._checks.get(name);
  }
}

const serverReadiness = new ServerReadinessImpl();

export function getServerReadiness(): { isReady: boolean; checks: Record<string, ReadinessCheck> } {
  return {
    isReady: serverReadiness.isReady,
    checks: serverReadiness.checks
  };
}

export function getReadinessTracker() {
  return serverReadiness;
}

export function markReadinessCheck(name: string, ready: boolean, error?: string) {
  const check = serverReadiness.getCheck(name);
  if (check) {
    check.ready = ready;
    check.error = error;
  }
}