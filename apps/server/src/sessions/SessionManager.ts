/**
 * Session Manager - Manages agent sessions
 * Inspired by t3code's orchestration but adapted for GAH
 */

import { generateSessionId, GAHError } from '@git-agent-harness/shared';
import { getRustBackendProxy } from '../rustBackend.js';
import { getProviderRegistry } from '../provider/ProviderRegistry.js';
import { getServerPushBus } from '../serverPushBus.js';
import type {
  Session, 
  SessionId, 
  ProviderKind, 
  ProviderInstanceId,
  SessionStatus
} from '@git-agent-harness/contracts';

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
    
    try {
      // Try to start the session via Rust backend if available
      const rustBackend = getRustBackendProxy();
      
      if (rustBackend.isBackendReady()) {
        // For now, simulate starting a session
        // In a real implementation, this would call the Rust backend
        console.log(`Starting session ${sessionId} with ${options.providerKind}`);
        
        // Simulate session starting
        await new Promise(resolve => setTimeout(resolve, 1000));
        
        session.status = 'running';
        this.sessions.set(sessionId, session);
        
        // Notify about session start
        getServerPushBus().publish({
          type: 'session.started',
          session
        });
        
        return session;
      } else {
        // TypeScript-only mode
        console.log(`Starting session ${sessionId} in TypeScript mode with ${options.providerKind}`);
        
        // Simulate session starting
        await new Promise(resolve => setTimeout(resolve, 500));
        
        session.status = 'running';
        this.sessions.set(sessionId, session);
        
        // Notify about session start
        getServerPushBus().publish({
          type: 'session.started',
          session
        });
        
        return session;
      }
      
    } catch (error) {
      session.status = 'error';
      session.error = error instanceof Error ? error.message : String(error);
      session.endedAt = new Date().toISOString();
      this.sessions.set(sessionId, session);
      
      throw error;
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
      // Try to stop via Rust backend if available
      const rustBackend = getRustBackendProxy();
      
      if (rustBackend.isBackendReady()) {
        // Simulate stopping
        await rustBackend.sendCommand(`session stop ${sessionId}`);
      }
      
      // Update session status
      session.status = 'stopping';
      this.sessions.set(sessionId, session);
      
      // Simulate graceful stop
      await new Promise(resolve => setTimeout(resolve, 500));
      
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
      
    } catch (error) {
      session.status = 'error';
      session.error = error instanceof Error ? error.message : String(error);
      session.endedAt = new Date().toISOString();
      this.sessions.set(sessionId, session);
      
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
    
    const rustBackend = getRustBackendProxy();
    
    if (rustBackend.isBackendReady()) {
      // Send command via Rust backend
      await rustBackend.sendCommand(`session ${sessionId} command ${command}`);
    } else {
      // TypeScript mode - just log the command
      console.log(`Session ${sessionId} command: ${command}`);
      
      // Add to output buffer
      const buffers = this.outputBuffers.get(sessionId);
      if (buffers) {
        buffers.stdout.push(`> ${command}`);
        
        // Simulate command output
        setTimeout(() => {
          buffers.stdout.push(`Command executed: ${command}`);
          
          getServerPushBus().publish({
            type: 'session.stdout',
            sessionId,
            data: `Command executed: ${command}\n`,
            timestamp: Date.now()
          });
        }, 100);
      }
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