/**
 * Session Manager - Manages agent sessions
 * Inspired by t3code's orchestration but adapted for GAH
 * 
 * After TICKET-113: Sessions now run actual `gah dispatch` commands via gahCli.ts
 * and stream the real output as session.stdout messages.
 */

import { generateSessionId, GAHError } from '@git-agent-harness/shared';
import { getProviderRegistry } from '../provider/ProviderRegistry.js';
import { getServerPushBus } from '../serverPushBus.js';
import { runDispatch, type DispatchOptions, type DispatchResult } from '../gahCli.js';
import type {
  Session, 
  SessionId, 
  ProviderKind, 
  ProviderInstanceId,
  SessionStatus
} from '@git-agent-harness/contracts';

type SessionOptions = {
  // GAH profile id (config.toml's [profiles.<id>]) -- NOT a backend name.
  // Required: there's no sane default to guess from providerKind/repo.
  profile: string;
  providerKind: ProviderKind;
  instanceId: ProviderInstanceId;
  repo: string;
  branch?: string;
  target?: string;
  mode: string;
  backend?: string;
  model?: string;
  budget?: number;
  dryRun?: boolean;
  retries?: number;
  allowDraftFail?: boolean;
  prod?: boolean;
  allowUnknownRedBaseline?: boolean;
  escalate?: boolean;
  hostId?: string;
};

// Active dispatch processes tracked by sessionId
class ActiveDispatch {
  constructor(
    public readonly sessionId: SessionId,
    public readonly process: Promise<DispatchResult>,
    public readonly cancel: () => void
  ) {}
}

class SessionManagerImpl {
  private sessions: Map<SessionId, Session> = new Map();
  private pendingSessions: Map<string, Promise<Session>> = new Map();
  private outputBuffers: Map<SessionId, { stdout: string[]; stderr: string[] }> = new Map();
  private activeDispatches: Map<SessionId, ActiveDispatch> = new Map();
  
  constructor() {
    // Set up periodic session cleanup
    setInterval(() => this.cleanupFinishedSessions(), 60000);
  }
  
  async startSession(options: SessionOptions): Promise<Session> {
    const sessionId = generateSessionId();
    const providerRegistry = getProviderRegistry();
    
    // Check if provider is available
    if (!providerRegistry.isProviderAvailable(options.providerKind)) {
      throw new GAHError(
        `Provider ${options.providerKind} is not available`,
        'PROVIDER_NOT_AVAILABLE'
      );
    }
    
    // Create initial session
    const session: Session = {
      id: sessionId,
      providerKind: options.providerKind,
      instanceId: options.instanceId,
      status: 'starting',
      startedAt: new Date().toISOString(),
      repo: options.repo,
      branch: options.branch,
      target: options.target,
      mode: options.mode,
      backend: options.backend,
      model: options.model,
      budget: options.budget,
      hostId: options.hostId
    };
    
    this.sessions.set(sessionId, session);
    this.outputBuffers.set(sessionId, { stdout: [], stderr: [] });
    
    // Notify about session start immediately
    getServerPushBus().publish({
      type: 'session.started',
      session
    });
    
    // Prepare dispatch options
    const dispatchOptions: DispatchOptions = {
      profile: options.profile,
      mode: options.mode,
      backend: options.backend,
      target: options.target,
      branch: options.branch,
      model: options.model,
      budget: options.budget,
      dryRun: options.dryRun,
      retries: options.retries,
      allowDraftFail: options.allowDraftFail,
      prod: options.prod,
      allowUnknownRedBaseline: options.allowUnknownRedBaseline,
      escalate: options.escalate
    };
    
    // Start the actual gah dispatch process
    const dispatchPromise = this.runDispatchProcess(sessionId, dispatchOptions);
    
    // Store the active dispatch so we can potentially cancel it
    this.activeDispatches.set(sessionId, new ActiveDispatch(
      sessionId,
      dispatchPromise,
      () => {
        // Cancel logic would go here
        // For now, we don't have a way to cancel a running dispatch
        console.log(`Cancel requested for session ${sessionId}`);
      }
    ));
    
    // Wait for dispatch to complete (but don't block the session start)
    dispatchPromise.then((result) => {
      this.handleDispatchComplete(sessionId, result);
    }).catch((error) => {
      this.handleDispatchError(sessionId, error);
    });
    
    // Return the session immediately - it's now running
    session.status = 'running';
    this.sessions.set(sessionId, session);
    
    return session;
  }
  
  /**
   * Run the actual gah dispatch process and stream output
   */
  private async runDispatchProcess(
    sessionId: SessionId,
    options: DispatchOptions
  ): Promise<DispatchResult> {
    return runDispatch(options, (line: string) => {
      // Forward each line as session.stdout message
      this.addSessionOutput(sessionId, line, false);
    });
  }
  
  /**
   * Handle completion of a dispatch process
   */
  private handleDispatchComplete(sessionId: SessionId, result: DispatchResult): void {
    const session = this.sessions.get(sessionId);
    if (!session) return;
    
    this.activeDispatches.delete(sessionId);
    
    if (result.exitCode === 0) {
      session.status = 'stopped';
      session.endedAt = new Date().toISOString();
      this.sessions.set(sessionId, session);
      
      getServerPushBus().publish({
        type: 'session.stopped',
        session
      });
    } else {
      session.status = 'error';
      session.error = `Dispatch failed with exit code ${result.exitCode}`;
      if (result.stderr) {
        session.error = result.stderr;
      }
      session.endedAt = new Date().toISOString();
      this.sessions.set(sessionId, session);
      
      getServerPushBus().publish({
        type: 'session.stopped',
        session
      });
    }
  }
  
  /**
   * Handle error in dispatch process
   */
  private handleDispatchError(sessionId: SessionId, error: unknown): void {
    const session = this.sessions.get(sessionId);
    if (!session) return;
    
    this.activeDispatches.delete(sessionId);
    
    session.status = 'error';
    session.error = error instanceof Error ? error.message : String(error);
    session.endedAt = new Date().toISOString();
    this.sessions.set(sessionId, session);
    
    getServerPushBus().publish({
      type: 'session.stopped',
      session
    });
  }
  
  async stopSession(sessionId: SessionId): Promise<Session> {
    const session = this.sessions.get(sessionId);
    
    if (!session) {
      throw new GAHError(`Session ${sessionId} not found`, 'SESSION_NOT_FOUND');
    }
    
    if (session.status === 'stopped' || session.status === 'stopping') {
      return session;
    }
    
    // If there's an active dispatch, cancel it
    const activeDispatch = this.activeDispatches.get(sessionId);
    if (activeDispatch) {
      activeDispatch.cancel();
      this.activeDispatches.delete(sessionId);
    }
    
    // Update session status
    session.status = 'stopping';
    this.sessions.set(sessionId, session);
    
    // Notify about session stop
    getServerPushBus().publish({
      type: 'session.status',
      session
    });
    
    // Mark as stopped after a brief delay
    await new Promise(resolve => setTimeout(resolve, 100));
    
    session.status = 'stopped';
    session.endedAt = new Date().toISOString();
    this.sessions.set(sessionId, session);
    
    // Clean up output buffers
    this.outputBuffers.delete(sessionId);
    
    // Notify about session stop
    getServerPushBus().publish({
      type: 'session.stopped',
      session
    });
    
    return session;
  }
  
  async sendCommand(sessionId: SessionId, command: string): Promise<void> {
    const session = this.sessions.get(sessionId);
    
    if (!session) {
      throw new GAHError(`Session ${sessionId} not found`, 'SESSION_NOT_FOUND');
    }
    
    if (session.status !== 'running') {
      throw new GAHError(
        `Cannot send command to session in ${session.status} state`,
        'SESSION_NOT_RUNNING'
      );
    }
    
    // For now, just log the command and add it to the output
    // In a future implementation, this could be sent to a running dispatch process
    console.log(`Session ${sessionId} command: ${command}`);
    
    // Add to output buffer
    const buffers = this.outputBuffers.get(sessionId);
    if (buffers) {
      buffers.stdout.push(`> ${command}`);
      
      // Publish the command to the push bus
      getServerPushBus().publish({
        type: 'session.stdout',
        sessionId,
        data: `> ${command}\n`,
        timestamp: Date.now()
      });
    }
  }
  
  addSessionOutput(sessionId: SessionId, data: string, isStderr: boolean = false): void {
    const buffers = this.outputBuffers.get(sessionId);
    if (buffers) {
      if (isStderr) {
        buffers.stderr.push(data);
      } else {
        buffers.stdout.push(data);
      }
      
      // Publish to push bus
      getServerPushBus().publish({
        type: isStderr ? 'session.stderr' : 'session.stdout',
        sessionId,
        data,
        timestamp: Date.now()
      });
    }
  }
  
  addRemoteSession(session: Session): void {
    this.sessions.set(session.id, session);
  }

  getSession(sessionId: SessionId): Session | undefined {
    return this.sessions.get(sessionId);
  }
  
  getAllSessions(): Session[] {
    return Array.from(this.sessions.values());
  }
  
  getSessionsByProvider(providerKind: ProviderKind): Session[] {
    return Array.from(this.sessions.values())
      .filter(session => session.providerKind === providerKind);
  }
  
  getSessionsByStatus(status: SessionStatus): Session[] {
    return Array.from(this.sessions.values())
      .filter(session => session.status === status);
  }
  
  getActiveSessions(): Session[] {
    return Array.from(this.sessions.values())
      .filter(session => ['starting', 'running'].includes(session.status));
  }
  
  getSessionOutput(sessionId: SessionId): { stdout: string; stderr: string } | undefined {
    const buffers = this.outputBuffers.get(sessionId);
    if (buffers) {
      return {
        stdout: buffers.stdout.join('\n'),
        stderr: buffers.stderr.join('\n')
      };
    }
    return undefined;
  }
  
  private cleanupFinishedSessions(): void {
    const now = Date.now();
    const finishedSessions: SessionId[] = [];
    
    for (const [sessionId, session] of this.sessions) {
      if (session.status === 'stopped' || session.status === 'error') {
        if (session.endedAt) {
          const endedAt = new Date(session.endedAt).getTime();
          const age = now - endedAt;
          
          // Clean up sessions older than 1 hour
          if (age > 60 * 60 * 1000) {
            finishedSessions.push(sessionId);
          }
        }
      }
    }
    
    for (const sessionId of finishedSessions) {
      this.sessions.delete(sessionId);
      this.outputBuffers.delete(sessionId);
    }
    
    if (finishedSessions.length > 0) {
      console.log(`Cleaned up ${finishedSessions.length} finished sessions`);
    }
  }
}

const sessionManager = new SessionManagerImpl();

export function getSessionManager(): SessionManagerImpl {
  return sessionManager;
}

export function createSessionManager(): SessionManagerImpl {
  return new SessionManagerImpl();
}