/**
 * WebSocket server handler
 * Inspired by t3code's wsServer.ts
 */

import { WebSocket, WebSocketServer } from 'ws';
import { SERVER_VERSION } from './server.js';
import { createServerPushBus } from './serverPushBus.js';
import { getProviderRegistry } from './provider/ProviderRegistry.js';
import { getSessionManager } from './sessions/SessionManager.js';
import { generateRequestId, GAHError, createErrorResponse } from '@git-agent-harness/shared';
import type {
  ServerMessage, 
  ClientMessage, 
  ClientCapabilities,
  Session,
  ProviderStatus,
  ProviderInstanceId,
  ProviderKind
} from '@git-agent-harness/contracts';

// Session store for tracking active WebSocket connections
class WebSocketSessionStore {
  private sessions: Map<WebSocket, { clientVersion: string; capabilities: ClientCapabilities }> = new Map();
  
  add(ws: WebSocket, clientVersion: string, capabilities: ClientCapabilities) {
    this.sessions.set(ws, { clientVersion, capabilities });
  }
  
  remove(ws: WebSocket) {
    this.sessions.delete(ws);
  }
  
  get(ws: WebSocket) {
    return this.sessions.get(ws);
  }
  
  getAll() {
    return Array.from(this.sessions.entries());
  }
  
  broadcast(message: ServerMessage, exclude?: WebSocket) {
    const messageStr = JSON.stringify(message);
    for (const [ws] of this.sessions) {
      if (ws !== exclude && ws.readyState === WebSocket.OPEN) {
        try {
          ws.send(messageStr);
        } catch (error) {
          console.error('Failed to send message to client:', error);
        }
      }
    }
  }
}

const sessionStore = new WebSocketSessionStore();
const pushBus = createServerPushBus();

export function createWebSocketHandler(wss: WebSocketServer) {
  wss.on('connection', (ws: WebSocket, req) => {
    console.log('WebSocket client connected');
    
    let clientInfo: { clientVersion: string; capabilities: ClientCapabilities } | null = null;
    let isAuthenticated = false;
    
    ws.on('message', async (data: WebSocket.RawData) => {
      try {
        const message = JSON.parse(data.toString()) as ClientMessage;
        await handleClientMessage(ws, message);
      } catch (error) {
        console.error('Failed to parse WebSocket message:', error);
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({
            type: 'error' as const,
            error: `Failed to parse message: ${error instanceof Error ? error.message : String(error)}`,
            requestId: generateRequestId()
          }));
        }
      }
    });
    
    ws.on('close', () => {
      console.log('WebSocket client disconnected');
      if (clientInfo) {
        sessionStore.remove(ws);
      }
    });
    
    ws.on('error', (error) => {
      console.error('WebSocket error:', error);
    });
    
    // Send welcome message after a brief delay to allow client to set up handlers
    setTimeout(() => {
      if (ws.readyState === WebSocket.OPEN) {
        sendWelcomeMessage(ws);
      }
    }, 100);
  });
  
  // Set up push bus to broadcast to all connected clients
  pushBus.subscribe((message: ServerMessage) => {
    sessionStore.broadcast(message);
  });
}

async function handleClientMessage(ws: WebSocket, message: ClientMessage) {
  // Extract requestId if available in the message type
  const requestId = 'requestId' in message && message.requestId ? message.requestId : generateRequestId();
  
  switch (message.type) {
    case 'client.hello':
      // Store client info
      sessionStore.add(ws, message.clientVersion, message.capabilities);
      console.log(`Client hello from ${message.clientVersion}`);
      break;
      
    case 'session.start':
      await handleStartSession(ws, message, requestId);
      break;
      
    case 'session.stop':
      await handleStopSession(ws, message, requestId);
      break;
      
    case 'session.sendCommand':
      await handleSendCommand(ws, message, requestId);
      break;
      
    case 'provider.refresh':
      await handleProviderRefresh(ws, message, requestId);
      break;
      
    case 'provider.list':
      await handleProviderList(ws, message, requestId);
      break;
      
    case 'ping':
      // Respond to ping
      ws.send(JSON.stringify({
        type: 'server.ping' as const,
        timestamp: Date.now()
      }));
      break;
      
    default:
      throw new GAHError(`Unknown message type: ${(message as any).type}`, 'UNKNOWN_MESSAGE_TYPE');
  }
}

async function handleStartSession(ws: WebSocket, message: Extract<ClientMessage, { type: 'session.start' }>, requestId: string) {
  const sessionManager = getSessionManager();
  
  try {
    const session = await sessionManager.startSession({
      profile: message.profile,
      providerKind: message.providerKind,
      instanceId: message.instanceId,
      repo: message.repo,
      branch: message.branch,
      target: message.target,
      mode: message.mode,
      backend: message.backend,
      model: message.model,
      budget: message.budget
    });
    
    // Notify all clients about new session
    pushBus.publish({
      type: 'session.started',
      session
    });
    
    // Send success response
    ws.send(JSON.stringify({
      type: 'session.started' as const,
      session
    }));
    
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleStopSession(ws: WebSocket, message: Extract<ClientMessage, { type: 'session.stop' }>, requestId: string) {
  const sessionManager = getSessionManager();
  
  try {
    const session = await sessionManager.stopSession(message.sessionId);
    
    pushBus.publish({
      type: 'session.stopped',
      session
    });
    
    ws.send(JSON.stringify({
      type: 'session.stopped' as const,
      session
    }));
    
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleSendCommand(ws: WebSocket, message: Extract<ClientMessage, { type: 'session.sendCommand' }>, requestId: string) {
  const sessionManager = getSessionManager();
  
  try {
    await sessionManager.sendCommand(message.sessionId, message.command);
    
    ws.send(JSON.stringify({
      type: 'session.status' as const,
      session: await sessionManager.getSession(message.sessionId)
    }));
    
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleProviderRefresh(ws: WebSocket, message: Extract<ClientMessage, { type: 'provider.refresh' }>, requestId: string) {
  try {
    const providerRegistry = getProviderRegistry();
    // Extract provider kind from instanceId (format: "provider_instance_0")
    const providerKind = message.instanceId.split('_')[0] as ProviderKind;
    const status = await providerRegistry.refreshProviderStatus(providerKind);
    
    pushBus.publish({
      type: 'provider.statusChanged',
      instanceId: message.instanceId,
      status
    });
    
    ws.send(JSON.stringify({
      type: 'provider.statusChanged' as const,
      instanceId: message.instanceId,
      status
    }));
    
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleProviderList(ws: WebSocket, message: Extract<ClientMessage, { type: 'provider.list' }>, requestId: string) {
  try {
    const providerRegistry = getProviderRegistry();
    const providers = providerRegistry.getAllProviderStatuses();
    
    ws.send(JSON.stringify({
      type: 'provider.listUpdated' as const,
      providers
    }));
    
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

function sendWelcomeMessage(ws: WebSocket) {
  try {
    const providerRegistry = getProviderRegistry();
    const sessionManager = getSessionManager();
    
    const serverProviderCatalog = {
      providers: providerRegistry.getProviderInstances()
    };
    
    const sessions = sessionManager.getAllSessions();
    const providers = providerRegistry.getAllProviderStatuses();
    
    const welcomeMessage: ServerMessage = {
      type: 'server.welcome',
      serverVersion: SERVER_VERSION,
      serverProviderCatalog,
      sessions,
      providers
    };
    
    ws.send(JSON.stringify(welcomeMessage));
    
  } catch (error) {
    console.error('Failed to send welcome message:', error);
    ws.send(JSON.stringify({
      type: 'error' as const,
      error: 'Failed to initialize server state',
      requestId: generateRequestId()
    }));
  }
}

export { WebSocketSessionStore, sessionStore };