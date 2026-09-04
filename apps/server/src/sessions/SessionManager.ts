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
import { runDispatchCancellable, type DispatchOptions, type DispatchResult, type CancellableDispatch, type CancellationResult } from '../gahCli.js';
import type {
  Session, 
  SessionId, 
  ProviderKind, 
  ProviderInstanceId,
  SessionStatus
} from '@git-agent-harness/contracts';

export type SessionOptions = {
  // GAH profile id (config.toml's [profiles.<id>]) -- NOT a backend name.
  // Required: there's no sane default to guess from providerKind/repo.
  profile: string;
  providerKind: ProviderKind;
  instanceId: ProviderInstanceId;
  repo: string;
  branch?: string;
  target?: string;
  mr?: string;
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
  requestId?: string;
};

type ProviderRegistryLike = {
  isProviderAvailable(kind: ProviderKind): boolean;
};

type PushBusLike = {
  publish(message: Parameters<ReturnType<typeof getServerPushBus>['publish']>[0]): void;
};

type DispatchRunner = typeof runDispatchCancellable;

type SessionManagerDeps = {
  providerRegistry?: ProviderRegistryLike;
  pushBus?: PushBusLike;
  dispatchRunner?: DispatchRunner;
  disableCleanupTimer?: boolean;
};

// Active dispatch processes tracked by sessionId
class ActiveDispatch {
  constructor(
    public readonly sessionId: SessionId,
    public readonly cancellableDispatch: CancellableDispatch,
    public readonly cancel: () => Promise<void>
  ) {}
}

class SessionManagerImpl {
  private sessions: Map<SessionId, Session> = new Map();
  private requestToSessionId: Map<string, SessionId> = new Map();
  private pendingSessions: Map<string, Promise<Session>> = new Map();
  private outputBuffers: Map<SessionId, { stdout: string[]; stderr: string[] }> = new Map();
  private activeDispatches: Map<SessionId, ActiveDispatch> = new Map();
  private profileDispatchTails: Map<string, Promise<void>> = new Map();
  private cancelledSessions: Set<SessionId> = new Set();
  private providerRegistry: ProviderRegistryLike;
  private pushBus: PushBusLike;
  private dispatchRunner: DispatchRunner;
  
  constructor(deps: SessionManagerDeps = {}) {
    this.providerRegistry = deps.providerRegistry ?? getProviderRegistry();
    this.pushBus = deps.pushBus ?? getServerPushBus();
    this.dispatchRunner = deps.dispatchRunner ?? runDispatchCancellable;

    // Set up periodic session cleanup. unref() so this housekeeping timer
    // never keeps the process (or a test importing this singleton) alive.
    if (!deps.disableCleanupTimer) {
      const timer = setInterval(() => this.cleanupFinishedSessions(), 60000);
      timer.unref?.();
    }
  }
  async startSession(options: SessionOptions): Promise<Session> {
    const { requestId } = options;
    if (requestId) {
      const pending = this.pendingSessions.get(requestId);
      if (pending) {
        return pending;
      }

      const existingSessionId = this.requestToSessionId.get(requestId);
      if (existingSessionId) {
        const existingSession = this.sessions.get(existingSessionId);
        if (existingSession) {
          return existingSession;
        }
        this.requestToSessionId.delete(requestId);
      }
    }

    const startPromise = this.startSessionFresh(options);
    if (requestId) {
      this.pendingSessions.set(requestId, startPromise);
    }
    try {
      const session = await startPromise;
      if (requestId) {
        this.requestToSessionId.set(requestId, session.id);
      }
      return session;
    } finally {
      if (requestId) {
        this.pendingSessions.delete(requestId);
      }
    }
  }

  private async startSessionFresh(options: SessionOptions): Promise<Session> {
    const sessionId = generateSessionId();

    // Check if provider is available
    if (!this.providerRegistry.isProviderAvailable(options.providerKind)) {
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
      mr: options.mr,
      mode: options.mode,
      backend: options.backend,
      model: options.model,
      budget: options.budget
    };
    
    this.sessions.set(sessionId, session);
    this.outputBuffers.set(sessionId, { stdout: [], stderr: [] });
    
    // Notify about session start immediately
    this.pushBus.publish({
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
      mr: options.mr,
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
    const cancellableDispatch = this.startDispatchProcess(sessionId, dispatchOptions);
    
    // Store the active dispatch so we can potentially cancel it
    this.activeDispatches.set(sessionId, new ActiveDispatch(
      sessionId,
      cancellableDispatch,
      async () => {
        await cancellableDispatch.cancel();
      }
    ));
    
    // Wait for dispatch to complete (but don't block the session start)
    cancellableDispatch.promise.then((result) => {
      this.handleDispatchComplete(sessionId, result);
    }).catch((error) => {
      this.handleDispatchError(sessionId, error);
    });
    
    // Return immediately. runDispatchProcess promotes it after the profile
    // queue grants this session the CLI slot.
    return session;
  }
  
  /**
   * Run the actual gah dispatch process and stream output
   */
  private startDispatchProcess(
    sessionId: SessionId,
    options: DispatchOptions
  ): CancellableDispatch {
    const previous = this.profileDispatchTails.get(options.profile) ?? Promise.resolve();
    let release: () => void;
    const slot = new Promise<void>((resolve) => { release = resolve; });
    const tail = previous.catch(() => undefined).then(() => slot);
    this.profileDispatchTails.set(options.profile, tail);

    // Create a deferred cancellable dispatch that will start after the previous one completes
    let actualDispatch: CancellableDispatch | null = null;

    const startAfterPrevious = async () => {
      await previous.catch(() => undefined);

      // Check if this session was cancelled while waiting
      if (this.cancelledSessions.has(sessionId)) {
        this.cancelledSessions.delete(sessionId);
        return {
          promise: Promise.resolve({ exitCode: -1, stderr: 'Session was cancelled before starting' }),
          cancel: async () => ({ cancelled: true }),
        } as CancellableDispatch;
      }

      // Promote status now that this session owns the profile slot.
      const session = this.sessions.get(sessionId);
      if (session?.status === 'starting') {
        session.status = 'running';
        this.sessions.set(sessionId, session);
        this.pushBus.publish({ type: 'session.status', session });
      }


      actualDispatch = this.dispatchRunner(options, (line: string) => {
        // Forward each line as session.stdout message
        this.addSessionOutput(sessionId, line, false);
      });
      return actualDispatch;
    };
    
    // Start the process after previous completes
    const dispatchPromise = startAfterPrevious().then(dispatch => dispatch.promise);
    
    const promise = new Promise<DispatchResult>(async (resolve, reject) => {
      try {
        const result = await dispatchPromise;
        resolve(result);
      } catch (error) {
        reject(error);
      } finally {
        release();
        if (this.profileDispatchTails.get(options.profile) === tail) {
          this.profileDispatchTails.delete(options.profile);
        }
      }
    });
    
    const cancel = async (): Promise<CancellationResult> => {
      if (actualDispatch) {
        await actualDispatch.cancel();
      }
      release();
      if (this.profileDispatchTails.get(options.profile) === tail) {
        this.profileDispatchTails.delete(options.profile);
      }
      return { cancelled: true };
    };
    
    return { promise, cancel };
  }
  
  /**
   * Handle completion of a dispatch process
   */
  private handleDispatchComplete(sessionId: SessionId, result: DispatchResult): void {
    const session = this.sessions.get(sessionId);
    if (!session) return;
    
    this.activeDispatches.delete(sessionId);
    
    // Do not overwrite terminal state if session was already stopped/stopping
    if (session.status === 'stopped' || session.status === 'stopping' || session.status === 'error') {
      // Session was already cancelled or stopped, ignore late completion
      return;
    }
    
    if (result.exitCode === 0) {
      session.status = 'stopped';
      session.endedAt = new Date().toISOString();
      this.sessions.set(sessionId, session);
      
      this.pushBus.publish({
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
      
      this.pushBus.publish({
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
    
    // Do not overwrite terminal state if session was already stopped/stopping
    if (session.status === 'stopped' || session.status === 'stopping' || session.status === 'error') {
      // Session was already cancelled or stopped, ignore late error
      return;
    }
    
    session.status = 'error';
    session.error = error instanceof Error ? error.message : String(error);
    session.endedAt = new Date().toISOString();
    this.sessions.set(sessionId, session);
    
    this.pushBus.publish({
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
    
    // Mark session as being cancelled to prevent queued sessions from starting
    this.cancelledSessions.add(sessionId);
    
    // Update session status to stopping
    session.status = 'stopping';
    this.sessions.set(sessionId, session);
    
    // Notify about session stop request
    this.pushBus.publish({
      type: 'session.status',
      session
    });

    // If there's an active dispatch, cancel it and wait for completion
    const activeDispatch = this.activeDispatches.get(sessionId);
    if (activeDispatch) {
      this.activeDispatches.delete(sessionId);
      
      try {
        // Wait for the cancellation to complete with a timeout
        const cancelTimeout = new Promise<{ timedOut: boolean }>((resolve) => {
          setTimeout(() => resolve({ timedOut: true }), 30000); // 30 second timeout
        });
        
        const cancelResult = await Promise.race([
          activeDispatch.cancel().then(() => ({ timedOut: false })),
          cancelTimeout
        ]);
        
        if (cancelResult.timedOut) {
          // If cancellation timed out, mark as error
          session.status = 'error';
          session.error = 'Session cancellation timed out';
        } else {
          // Cancellation completed successfully
          session.status = 'stopped';
        }
      } catch (error) {
        // If there's an error during cancellation, still mark as stopped
        session.status = 'stopped';
        session.error = `Error during cancellation: ${error instanceof Error ? error.message : String(error)}`;
      }
    } else {
      // No active dispatch, just mark as stopped
      session.status = 'stopped';
    }
    
    session.endedAt = new Date().toISOString();
    this.sessions.set(sessionId, session);
    
    // Clean up output buffers
    this.outputBuffers.delete(sessionId);

    // Notify about session stop - only after cancellation completes or times out
    this.pushBus.publish({
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
      this.pushBus.publish({
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
      this.pushBus.publish({
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
      const requestIdsToRemove = Array.from(this.requestToSessionId.entries())
        .filter(([, mappedSessionId]) => mappedSessionId === sessionId)
        .map(([requestId]) => requestId);
      for (const requestId of requestIdsToRemove) {
        this.requestToSessionId.delete(requestId);
        this.pendingSessions.delete(requestId);
      }
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

export function createSessionManager(deps?: SessionManagerDeps): SessionManagerImpl {
  return new SessionManagerImpl(deps);
}
