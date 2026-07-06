import React, { createContext, useContext, useEffect, useState, useCallback, ReactNode } from 'react';
import type { ServerMessage, ClientMessage, Session, ProviderInstance, ServerProviderCatalog, ProviderStatus, ProviderInstanceId } from '@git-agent-harness/contracts';

type WebSocketContextType = {
  socket: WebSocket | null;
  isConnected: boolean;
  isConnecting: boolean;
  error: string | null;
  messages: ServerMessage[];
  sessions: Session[];
  providers: ProviderInstance[];
  providerStatuses: Record<ProviderInstanceId, ProviderStatus>;
  serverProviderCatalog: ServerProviderCatalog | null;
  serverVersion: string | null;
  sendMessage: (message: ClientMessage) => void;
  reconnect: () => void;
  disconnect: () => void;
};

const WebSocketContext = createContext<WebSocketContextType | undefined>(undefined);

// Vite injects import.meta.env - use type assertion for TypeScript
const SERVER_WS_URL = (import.meta as unknown as { env: { VITE_WS_URL?: string } }).env?.VITE_WS_URL || 'ws://localhost:3773';

export function WebSocketProvider({ children }: { children: ReactNode }) {
  const [socket, setSocket] = useState<WebSocket | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [isConnecting, setIsConnecting] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [messages, setMessages] = useState<ServerMessage[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [providers, setProviders] = useState<ProviderInstance[]>([]);
  const [providerStatuses, setProviderStatuses] = useState<Record<ProviderInstanceId, ProviderStatus>>({});
  const [serverProviderCatalog, setServerProviderCatalog] = useState<ServerProviderCatalog | null>(null);
  const [serverVersion, setServerVersion] = useState<string | null>(null);

  const connect = useCallback(() => {
    setIsConnecting(true);
    setError(null);

    try {
      const newSocket = new WebSocket(SERVER_WS_URL);
      
      newSocket.onopen = () => {
        setIsConnected(true);
        setIsConnecting(false);
        setSocket(newSocket);
        
        // Send client hello
        newSocket.send(JSON.stringify({
          type: 'client.hello' as const,
          clientVersion: '0.1.0',
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
      };

      newSocket.onerror = (errorEvent: Event) => {
        // ErrorEvent has message property but is typed as Event
        const errorMessage = 'message' in errorEvent && typeof errorEvent.message === 'string' ? errorEvent.message : 'WebSocket connection error';
        setError(errorMessage);
        setIsConnecting(false);
      };

      newSocket.onmessage = (event) => {
        try {
          const message = JSON.parse(event.data) as ServerMessage;
          
          // Store the raw message
          setMessages(prev => [...prev.slice(-100), message]); // Keep last 100 messages
          
          // Handle different message types
          switch (message.type) {
            case 'server.welcome':
              setServerVersion(message.serverVersion);
              setServerProviderCatalog(message.serverProviderCatalog);
              setSessions(message.sessions);
              setProviderStatuses(message.providers);
              
              // Extract provider instances from catalog
              if (message.serverProviderCatalog?.providers) {
                setProviders(message.serverProviderCatalog.providers);
              }
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
            case 'session.stderr':
              // These are handled by the session detail views
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
              // Pong not needed, server just sends pings for keepalive
              break;
              
            case 'error':
              setError(message.error);
              break;
          }
        } catch (error) {
          console.error('Failed to parse WebSocket message:', error);
        }
      };

      // Set up keepalive
      const keepaliveInterval = setInterval(() => {
        if (newSocket.readyState === WebSocket.OPEN) {
          newSocket.send(JSON.stringify({
            type: 'ping' as const,
            requestId: `ping_${Date.now()}`,
            timestamp: Date.now()
          }));
        }
      }, 30000);

      // Clean up on unmount
      return () => {
        clearInterval(keepaliveInterval);
        if (newSocket.readyState === WebSocket.OPEN) {
          newSocket.close();
        }
      };
      
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
      setIsConnecting(false);
    }
  }, []);

  const sendMessage = useCallback((message: ClientMessage) => {
    if (socket && socket.readyState === WebSocket.OPEN) {
      try {
        socket.send(JSON.stringify(message));
      } catch (error) {
        setError(error instanceof Error ? error.message : String(error));
      }
    } else {
      setError('Not connected to server');
    }
  }, [socket]);

  const reconnect = useCallback(() => {
    // Close existing connection first
    if (socket) {
      socket.close();
      setSocket(null);
    }
    setIsConnected(false);
    setIsConnecting(true);
    connect();
  }, [socket, connect]);

  const disconnect = useCallback(() => {
    if (socket) {
      socket.close();
      setSocket(null);
      setIsConnected(false);
    }
  }, [socket]);

  // Initial connection
  useEffect(() => {
    connect();
    
    return () => {
      if (socket) {
        socket.close();
      }
    };
  }, [connect]);

  // Auto-reconnect when disconnected
  useEffect(() => {
    if (!isConnected && !isConnecting && !error && socket === null) {
      const timer = setTimeout(() => {
        reconnect();
      }, 5000);
      
      return () => clearTimeout(timer);
    }
  }, [isConnected, isConnecting, error, socket, reconnect]);

  const value = {
    socket,
    isConnected,
    isConnecting,
    error,
    messages,
    sessions,
    providers,
    providerStatuses,
    serverProviderCatalog,
    serverVersion,
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