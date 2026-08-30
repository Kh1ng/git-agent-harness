import React, { createContext, useContext, useEffect, useState, useCallback, useRef, ReactNode } from 'react';
import { useUiStore } from '../store/uiStore.js';
import type {
  ServerMessage,
  ClientMessage,
  Session,
  SessionId,
  ProviderInstance,
  ServerProviderCatalog,
  ProviderStatus,
  ProviderInstanceId,
  MergeRequest,
  AvailabilityScope,
  Blocker,
  StatusError,
  RecentLedgerSummary,
  ControllerActivity,
  DependencyBlocker
} from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';

export interface SessionOutput {
  stdout: string;
  stderr: string;
}

export interface WebSocketInboxEntry {
  /** Monotonic for this provider lifetime, including across reconnects. */
  id: number;
  message: ServerMessage;
}

type WebSocketContextType = {
  socket: WebSocket | null;
  isConnected: boolean;
  isConnecting: boolean;
  error: string | null;
  /** Bounded inbox. Consumers advance by entry id, never array position,
   * because older entries are evicted from the front. */
  messages: WebSocketInboxEntry[];
  sessions: Session[];
  /** Real per-line stdout/stderr streamed from the server, keyed by
   * session id -- see SessionManager.ts's getServerPushBus().publish()
   * calls, which this actually consumes now instead of discarding. */
  sessionOutput: Record<SessionId, SessionOutput>;
  providers: ProviderInstance[];
  providerStatuses: Record<ProviderInstanceId, ProviderStatus>;
  serverProviderCatalog: ServerProviderCatalog | null;
  // TICKET-157: per-backend "configured for this profile" signal.
  backendConfigured: Record<string, boolean>;
  serverVersion: string | null;
  profile: string | null;
  mergeRequests: MergeRequest[];
  availability: AvailabilityScope[];
  blockers: Blocker[];
  constraints: Blocker[];
  dependencyBlockers: DependencyBlocker[];
  errors: StatusError[];
  recentLedger: RecentLedgerSummary | null;
  controllerActivity: ControllerActivity[];
  /** Increments each time the socket re-establishes a connection after
   * having previously been connected (i.e. actual reconnects, not the
   * initial connect on mount). Pages use this to re-trigger a fresh REST
   * pull of whatever they depend on -- a dropped/restored connection
   * would otherwise leave a panel stale until its own timer or a manual
   * refresh fires. See useWsReconnectRefresh. */
  reconnectSeq: number;
  sendMessage: (message: ClientMessage) => void;
  reconnect: () => void;
  disconnect: () => void;
};

const WebSocketContext = createContext<WebSocketContextType | undefined>(undefined);
const MAX_INBOX_MESSAGES = 100;

// Vite injects import.meta.env - use type assertion for TypeScript
const SERVER_WS_BASE = (import.meta as unknown as { env: { VITE_WS_URL?: string } }).env?.VITE_WS_URL ||
  `${window.location.protocol === 'https:' ? 'wss:' : 'ws:'}//${window.location.host}/ws`;

function getWebSocketUrl(profile?: string | null): string {
  const url = new URL(SERVER_WS_BASE);
  if (profile) {
    url.searchParams.set('profile', profile);
  }
  return url.toString();
}

export function WebSocketProvider({ children }: { children: ReactNode }) {
  const profileOverride = useUiStore((state) => state.profileOverride);
  const [socket, setSocket] = useState<WebSocket | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [isConnecting, setIsConnecting] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [messages, setMessages] = useState<WebSocketInboxEntry[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionOutput, setSessionOutput] = useState<Record<SessionId, SessionOutput>>({});
  const [providers, setProviders] = useState<ProviderInstance[]>([]);
  const [providerStatuses, setProviderStatuses] = useState<Record<ProviderInstanceId, ProviderStatus>>({});
  const [serverProviderCatalog, setServerProviderCatalog] = useState<ServerProviderCatalog | null>(null);
  // TICKET-157: per-backend "configured for this profile" signal from
  // `configured_backend_path()`. Maps backend name -> configured bool.
  const [backendConfigured, setBackendConfigured] = useState<Record<string, boolean>>({});
  const [serverVersion, setServerVersion] = useState<string | null>(null);
  const [profile, setProfile] = useState<string | null>(null);
  const [mergeRequests, setMergeRequests] = useState<MergeRequest[]>([]);
  const [availability, setAvailability] = useState<AvailabilityScope[]>([]);
  const [blockers, setBlockers] = useState<Blocker[]>([]);
  const [constraints, setConstraints] = useState<Blocker[]>([]);
  const [dependencyBlockers, setDependencyBlockers] = useState<DependencyBlocker[]>([]);
  const [errors, setErrors] = useState<StatusError[]>([]);
  const [recentLedger, setRecentLedger] = useState<RecentLedgerSummary | null>(null);
  const [controllerActivity, setControllerActivity] = useState<ControllerActivity[]>([]);
  const [reconnectSeq, setReconnectSeq] = useState(0);
  const socketRef = useRef<WebSocket | null>(null);
  const connectionCleanupRef = useRef<(() => void) | null>(null);
  const hasConnectedOnceRef = useRef(false);
  const nextMessageIdRef = useRef(0);

  const activityProfile = profileOverride ?? profile ?? 'gah';
  useEffect(() => {
    let cancelled = false;
    const refreshActivity = async () => {
      try {
        const activity = await gahApi.getControllerActivity({ profile: activityProfile, since: '24h' });
        if (!cancelled) setControllerActivity(activity);
      } catch {
        // Status/event fetches remain available; activity is supplementary.
      }
    };
    refreshActivity();
    const timer = window.setInterval(refreshActivity, 5000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activityProfile]);

  const connect = useCallback(() => {
    connectionCleanupRef.current?.();
    connectionCleanupRef.current = null;
    socketRef.current = null;
    setSocket(null);
    setIsConnected(false);
    setIsConnecting(true);
    setError(null);

    try {
      const newSocket = new WebSocket(getWebSocketUrl(profileOverride));

      newSocket.onopen = () => {
        setIsConnected(true);
        setIsConnecting(false);
        setError(null);
        setSocket(newSocket);
        socketRef.current = newSocket;

        if (hasConnectedOnceRef.current) {
          setReconnectSeq((n) => n + 1);
        }
        hasConnectedOnceRef.current = true;

        newSocket.send(JSON.stringify({
          type: 'client.hello' as const,
          clientVersion: '0.1.0',
          profile: profileOverride ?? undefined,
          capabilities: {
            supportsTerminal: true,
            supportsNotifications: true,
            version: '0.1.0'
          }
        }));
      };

      newSocket.onclose = () => {
        setIsConnected(false);
        setIsConnecting(false);
        setSocket(null);
        socketRef.current = null;
      };

      newSocket.onerror = (errorEvent: Event) => {
        const errorMessage = 'message' in errorEvent && typeof errorEvent.message === 'string' ? errorEvent.message : 'WebSocket connection error';
        setError(errorMessage);
        setIsConnecting(false);
      };

      newSocket.onmessage = (event) => {
        try {
          const message = JSON.parse(event.data) as ServerMessage;

          const entry = { id: ++nextMessageIdRef.current, message };
          setMessages(prev => [...prev.slice(-(MAX_INBOX_MESSAGES - 1)), entry]);

          switch (message.type) {
            case 'server.welcome':
              setServerVersion(message.serverVersion);
              setServerProviderCatalog(message.serverProviderCatalog);
              setSessions(message.sessions);
              setProviderStatuses(message.providers);

              if (message.serverProviderCatalog?.providers) {
                setProviders(message.serverProviderCatalog.providers);
              }

              if (message.profile !== undefined) setProfile(message.profile);
              if (message.mergeRequests !== undefined) setMergeRequests(message.mergeRequests);
              if (message.availability !== undefined) setAvailability(message.availability);
              if (message.blockers !== undefined) setBlockers(message.blockers);
              if (message.constraints !== undefined) setConstraints(message.constraints);
              if (message.dependencyBlockers !== undefined) setDependencyBlockers(message.dependencyBlockers);
              if (message.errors !== undefined) setErrors(message.errors);
              if (message.recentLedger !== undefined) setRecentLedger(message.recentLedger);
              if (message.backendConfigured !== undefined) setBackendConfigured(message.backendConfigured);
              break;

            case 'session.started':
            case 'session.stopped':
            case 'session.status':
              setSessions(prev => {
                const existingIndex = prev.findIndex(s => s.id === message.session.id);
                if (existingIndex >= 0) {
                  return [
                    ...prev.slice(0, existingIndex),
                    message.session,
                    ...prev.slice(existingIndex + 1)
                  ];
                }
                return [...prev, message.session];
              });
              break;

            case 'session.stdout':
              setSessionOutput(prev => {
                const existing = prev[message.sessionId] ?? { stdout: '', stderr: '' };
                return {
                  ...prev,
                  [message.sessionId]: { ...existing, stdout: existing.stdout + message.data + '\n' }
                };
              });
              break;

            case 'session.stderr':
              setSessionOutput(prev => {
                const existing = prev[message.sessionId] ?? { stdout: '', stderr: '' };
                return {
                  ...prev,
                  [message.sessionId]: { ...existing, stderr: existing.stderr + message.data + '\n' }
                };
              });
              break;

            case 'provider.statusChanged':
              setProviderStatuses(prev => ({
                ...prev,
                [message.instanceId]: message.status
              }));
              break;

            case 'provider.listUpdated':
              setProviderStatuses(message.providers);
              break;

            case 'server.ping':
              break;

          }
        } catch (error) {
          console.error('Failed to parse WebSocket message:', error);
        }
      };

      const keepaliveInterval = setInterval(() => {
        if (newSocket.readyState === WebSocket.OPEN) {
          newSocket.send(JSON.stringify({
            type: 'ping' as const,
            requestId: `ping_${Date.now()}`,
            timestamp: Date.now()
          }));
        }
      }, 30000);

      connectionCleanupRef.current = () => {
        clearInterval(keepaliveInterval);
        newSocket.onopen = null;
        newSocket.onclose = null;
        newSocket.onerror = null;
        newSocket.onmessage = null;
        if (newSocket.readyState === WebSocket.OPEN || newSocket.readyState === WebSocket.CONNECTING) {
          newSocket.close();
        }
      };

    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
      setIsConnecting(false);
    }
  }, [profileOverride]);

  const sendMessage = useCallback((message: ClientMessage) => {
    const current = socketRef.current;
    if (current && current.readyState === WebSocket.OPEN) {
      try {
        current.send(JSON.stringify(message));
      } catch (error) {
        setError(error instanceof Error ? error.message : String(error));
      }
    } else {
      setError('Not connected to server');
    }
  }, []);

  const reconnect = useCallback(() => {
    if (socketRef.current) {
      socketRef.current.close();
      setSocket(null);
      socketRef.current = null;
    }
    setIsConnected(false);
    setIsConnecting(true);
    connect();
  }, [connect]);

  const disconnect = useCallback(() => {
    if (socketRef.current) {
      socketRef.current.close();
      setSocket(null);
      socketRef.current = null;
      setIsConnected(false);
    }
  }, []);

  useEffect(() => {
    connect();
    return () => {
      connectionCleanupRef.current?.();
      connectionCleanupRef.current = null;
    };
  }, [connect]);

  useEffect(() => {
    if (!isConnected && !isConnecting && socket === null) {
      const timer = setTimeout(() => {
        reconnect();
      }, 5000);

      return () => clearTimeout(timer);
    }
  }, [isConnected, isConnecting, error, socket, reconnect]);

  const value: WebSocketContextType = {
    socket,
    isConnected,
    isConnecting,
    error,
    messages,
    sessions,
    sessionOutput,
    providers,
    providerStatuses,
    serverProviderCatalog,
    backendConfigured,
    serverVersion,
    profile,
    mergeRequests,
    availability,
    blockers,
    constraints,
    dependencyBlockers,
    errors,
    recentLedger,
    controllerActivity,
    reconnectSeq,
    sendMessage,
    reconnect,
    disconnect
  };

  return (
    <WebSocketContext.Provider value={value}>
      {children}
    </WebSocketContext.Provider>
  );
}

export function useWebSocket() {
  const context = useContext(WebSocketContext);
  if (context === undefined) {
    throw new Error('useWebSocket must be used within a WebSocketProvider');
  }
  return context;
}
