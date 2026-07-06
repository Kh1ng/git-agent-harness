/**
 * Session Manager - Manages agent sessions
 * Inspired by t3code's orchestration but adapted for GAH
 * Updated per TICKET-113 to use GAH CLI directly instead of driver stubs
 */

import { generateSessionId, GAHError } from '@git-agent-harness/shared';
import { getProviderRegistry } from '../provider/ProviderRegistry.js';
import { getServerPushBus } from '../serverPushBus.js';
import { spawnDispatch, runStatus, isBackendAvailable, type StatusSnapshot } from '../gahCli.js';
import type {
  Session, 
  SessionId, 
  ProviderKind, 
  ProviderInstanceId,
  SessionStatus
} from '@git-agent-harness/contracts';
import { once } from 'node:events';
import { ChildProcessWithoutNullStreams } from 'node:child_process';

type SessionOptions = {
  providerKind: ProviderKind;
  instanceId: ProviderInstanceId;
  repo: string;
  branch?: string;
  target?: string;
  mode: string;
  backend?: string;
  model?: string;
  budget?: number;
};

class SessionManagerImpl {
  private sessions: Map<SessionId, Session> = new Map();
  private pendingSessions: Map<string, Promise<Session>> = new Map();
  private outputBuffers: Map<SessionId, { stdout: string[]; stderr: string[] }> = new Map();
  private dispatchProcesses: Map<SessionId, ChildProcessWithoutNullStreams> = new Map();
  
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
      budget: options.budget
    };
    
    this.sessions.set(sessionId, session);
    this.outputBuffers.set(sessionId, { stdout: [], stderr: [] });
    
    // Notify about session start immediately
    getServerPushBus().publish({
      type: 'session.started',
      session
    });
    
    // Start the actual gah dispatch command in the background
    // A session IS one gah dispatch invocation per TICKET-113
    this.startDispatchProcess(sessionId, options);
    
    return session;
  }
  
  /**
   * Start the actual gah dispatch process and stream its output
   */
  private async startDispatchProcess(sessionId: SessionId, options: SessionOptions): Promise<void> {
    const session = this.sessions.get(sessionId);
    if (!session) {
      console.warn(`Session ${sessionId} not found when starting dispatch`);
      return;
    }
    
    try {
      // Extract profile from providerKind - for now use the providerKind as profile
      // TODO: This should be configurable or mapped from provider to profile
      const profile = options.providerKind;
      const backend = options.backend || 'auto';
      const target = options.target || options.repo;
      
      if (!target) {
        throw new GAHError('No target specified for dispatch', 'NO_TARGET');
      }
      
      console.log(`[TICKET-113] Starting gah dispatch for session ${sessionId}: profile=${profile}, mode=${options.mode}, backend=${backend}, target=${target}`);
      
      // Update session status to running
      session.status = 'running';
      this.sessions.set(sessionId, session);
      
      // Spawn gah dispatch process and track it for later cancellation
      const process = spawnDispatch(
        profile,
        options.mode,
        backend,
        target,
        (line) => {
          // Forward each line as session.stdout message
          this.addSessionOutput(sessionId, line, false);
        },
        undefined,
        []
      );
      
      // Store the process for later cancellation
      this.dispatchProcesses.set(sessionId, process);
      
      // Wait for process to complete
      const [exitCode] = await once(process, 'exit');
      
      // Clean up process tracking
      this.dispatchProcesses.delete(sessionId);
      
      // Update session based on exit code
      if (exitCode === 0) {
        session.status = 'stopped';
      } else {
        session.status = 'error';
        session.error = `Dispatch failed with exit code ${exitCode}`;
      }
      session.endedAt = new Date().toISOString();
      this.sessions.set(sessionId, session);
      
      // Notify about session stop
      getServerPushBus().publish({
        type: 'session.stopped',
        session
      });
      
      console.log(`[TICKET-113] Session ${sessionId} completed with exit code ${exitCode}`);
      
    } catch (error) {
      const session = this.sessions.get(sessionId);
      this.dispatchProcesses.delete(sessionId);
      
      if (session) {
        session.status = 'error';
        session.error = error instanceof Error ? error.message : String(error);
        session.endedAt = new Date().toISOString();
        this.sessions.set(sessionId, session);
        
        getServerPushBus().publish({
          type: 'session.stopped',
          session
        });
        
        console.error(`[TICKET-113] Session ${sessionId} failed:`, error);
      }
    }
  }
  
  async stopSession(sessionId: SessionId): Promise<Session> {
    const session = this.sessions.get(sessionId);
    
    if (!session) {
      throw new GAHError(`Session ${sessionId} not found`, 'SESSION_NOT_FOUND');
    }
    
    if (session.status === 'stopped' || session.status === 'stopping') {
      return session;
    }
    
    try {
      // [TICKET-113] Stop the actual gah dispatch process
      const process = this.dispatchProcesses.get(sessionId);
      if (process) {
        console.log(`[TICKET-113] Killing dispatch process for session ${sessionId}`);
        process.kill('SIGTERM');
        
        // Wait a bit for graceful shutdown
        try {
          await new Promise((resolve) => {
            const timeout = setTimeout(resolve, 2000); // 2 second timeout
            process.on('exit', () => {
              clearTimeout(timeout);
              resolve(void 0);
            });
          });
        } catch {
          // Process already exited or timeout
        }
        
        // Force kill if still running
        if (!process.killed) {
          process.kill('SIGKILL');
        }
        
        this.dispatchProcesses.delete(sessionId);
      }
      
      // Update session status
      session.status = 'stopping';
      this.sessions.set(sessionId, session);
      
      // If process was already dead, mark as stopped
      if (!process || process.killed) {
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
      }
      
      return session;
      
    } catch (error) {
      const currentSession = this.sessions.get(sessionId);
      if (currentSession) {
        currentSession.status = 'error';
        currentSession.error = error instanceof Error ? error.message : String(error);
        currentSession.endedAt = new Date().toISOString();
        this.sessions.set(sessionId, currentSession);
      }
      
      throw error;
    }
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
    
    // [TICKET-113] With CLI-based dispatch, interactive commands are not supported
    // Each gah dispatch runs to completion as a separate process.
    // This is a known limitation of the current implementation.
    // For interactive sessions, the Rust WebSocket server (src/server.rs) would be needed,
    // but TICKET-113 explicitly excludes that from scope.
    console.log(`[TICKET-113] Session ${sessionId} command (not supported with CLI dispatch): ${command}`);
    
    // Add to output buffer
    const buffers = this.outputBuffers.get(sessionId);
    if (buffers) {
      buffers.stdout.push(`[Note: Interactive commands are not supported with CLI-based dispatch]`);
      buffers.stdout.push(`> ${command}`);
      
      getServerPushBus().publish({
        type: 'session.stdout',
        sessionId,
        data: `[Note: Interactive commands are not supported with CLI-based dispatch]\n> ${command}\n`,
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
      this.dispatchProcesses.delete(sessionId);
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