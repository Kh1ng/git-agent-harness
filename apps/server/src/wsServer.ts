/**
 * WebSocket server handler
 * Inspired by t3code's wsServer.ts
 */

import { WebSocket, WebSocketServer } from 'ws';
import { SERVER_VERSION } from './server.js';
import { createServerPushBus } from './serverPushBus.js';
import { getProviderRegistry } from './provider/ProviderRegistry.js';
import { getSessionManager } from './sessions/SessionManager.js';
import { createFleetDispatchCoordinator } from './fleetDispatch.js';
import { RegistryService } from './registryService.js';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import * as gahCli from './gahCli.js';
import { generateRequestId, GAHError, createErrorResponse } from '@git-agent-harness/shared';
import type {
  ServerMessage,
  ClientMessage,
  ClientCapabilities,
  Session,
  ProviderStatus,
  ProviderInstanceId,
  ProviderKind,
  MergeRequest,
  AvailabilityScope,
  Blocker,
  StatusError,
  RecentLedgerSummary
} from '@git-agent-harness/contracts';

// Session store for tracking active WebSocket connections
class WebSocketSessionStore {
  private sessions: Map<WebSocket, { clientVersion: string; capabilities: ClientCapabilities; profile: string }> = new Map();
  
  add(ws: WebSocket, clientVersion: string, capabilities: ClientCapabilities, profile: string) {
    this.sessions.set(ws, { clientVersion, capabilities, profile });
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
let fleetDispatch = createFleetDispatchCoordinator({
  registryService: new RegistryService(),
  pushBus,
  coordinatorIdentity: getCoordinatorIdentity(),
  localSessionManager: getSessionManager()
});

// Temporary storage for profile from query params, used before client.hello arrives
const pendingProfiles = new Map<WebSocket, string>();

export function createWebSocketHandler(
  wss: WebSocketServer,
  deps: {
    registryService?: RegistryService;
    coordinatorIdentity?: ReturnType<typeof getCoordinatorIdentity>;
  } = {}
) {
  fleetDispatch = createFleetDispatchCoordinator({
    registryService: deps.registryService ?? new RegistryService(),
    pushBus,
    coordinatorIdentity: deps.coordinatorIdentity ?? getCoordinatorIdentity(),
    localSessionManager: getSessionManager()
  });

  wss.on('connection', (ws: WebSocket, req) => {
    console.log('WebSocket client connected');
    
    let clientInfo: { clientVersion: string; capabilities: ClientCapabilities } | null = null;
    let isAuthenticated = false;
    
    // Extract profile from query parameters in the connection URL
    const url = new URL(req.url || '', `http://${req.headers.host || 'localhost'}`);
    const profileFromQuery = url.searchParams.get('profile') || null;
    
    // Store profile from query param temporarily until client.hello arrives
    if (profileFromQuery) {
      pendingProfiles.set(ws, profileFromQuery);
    }
    
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
      pendingProfiles.delete(ws);
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
      // Use profile from client.hello message, or fall back to query param from pendingProfiles, or default to 'gah'
      const pendingProfile = pendingProfiles.get(ws);
      const profile = message.profile ?? pendingProfile ?? 'gah';
      sessionStore.add(ws, message.clientVersion, message.capabilities, profile);
      // Clean up pending profile
      pendingProfiles.delete(ws);
      console.log(`Client hello from ${message.clientVersion} with profile: ${profile}`);
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
  try {
    const session = await fleetDispatch.startSession({
      requestId: message.requestId,
      nodeId: message.nodeId,
      coordinatorNodeId: message.coordinatorNodeId,
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
  try {
    const session = await fleetDispatch.stopSession(message.sessionId);

    ws.send(JSON.stringify({
      type: 'session.stopped' as const,
      session
    }));
    
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleSendCommand(ws: WebSocket, message: Extract<ClientMessage, { type: 'session.sendCommand' }>, requestId: string) {
  try {
    await fleetDispatch.sendCommand(message.sessionId, message.command);
    
    ws.send(JSON.stringify({
      type: 'session.status' as const,
      session: await fleetDispatch.getSession(message.sessionId)
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

async function sendWelcomeMessage(ws: WebSocket) {
  try {
    const providerRegistry = getProviderRegistry();

    const serverProviderCatalog = {
      providers: providerRegistry.getProviderInstances()
    };

    const sessions = fleetDispatch.getAllSessions();
    const providers = providerRegistry.getAllProviderStatuses();

    // Include real GAH data (TICKET-114) via the same gahCli.runStatus()
    // path TICKET-113 already wired up -- there's no separate
    // per-field ProviderRegistry accessor, `gah status --json` returns
    // all of this in one call.
    const defaultProfile = sessionStore.get(ws)?.profile ?? pendingProfiles.get(ws) ?? 'gah';
    let mergeRequests: MergeRequest[] = [];
    let availability: AvailabilityScope[] = [];
    let blockers: Blocker[] = [];
    let constraints: Blocker[] = [];
    let errors: StatusError[] = [];
    // recent_ledger is a single nullable summary, not an array -- it was
    // previously mistyped as unknown[] here (silently accepted at runtime
    // by JS, but wrong; DashboardPage already correctly treats it as an
    // object via `{recentLedger && ...}`).
    let recentLedger: RecentLedgerSummary | null = null;
    // TICKET-157: per-backend "configured for this profile" signal,
    // derived from the Rust harness `configured_backend_path()`. Maps a
    // backend name to whether it has a real implementation and is wired
    // for the active profile.
    let backendConfigured: Record<string, boolean> = {};
    try {
      const status = await gahCli.runStatus(defaultProfile);
      mergeRequests = status.merge_requests;
      availability = status.availability;
      blockers = status.blockers;
      constraints = status.constraints;
      errors = status.errors;
      recentLedger = status.recent_ledger;
      backendConfigured = status.backend_configured ?? {};
    } catch (statusError) {
      console.error('Failed to load gah status for welcome message:', statusError);
    }

    const welcomeMessage: ServerMessage & {
      profile?: string;
      mergeRequests?: MergeRequest[];
      availability?: AvailabilityScope[];
      blockers?: Blocker[];
      constraints?: Blocker[];
      errors?: StatusError[];
      recentLedger?: RecentLedgerSummary | null;
      backendConfigured?: Record<string, boolean>;
    } = {
      type: 'server.welcome',
      serverVersion: SERVER_VERSION,
      serverProviderCatalog,
      sessions,
      providers,
      profile: defaultProfile,
      mergeRequests,
      availability,
      blockers,
      constraints,
      errors,
      recentLedger,
      backendConfigured
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
