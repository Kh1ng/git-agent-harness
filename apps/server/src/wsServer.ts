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
import { sendManagerChatMessage, steerManagerChatTurn, cancelManagerChatTurn, getSessionView as getManagerChatSessionView, setChunkPublisher, setChatEventPublishers, setPreviewPublisher, listChatSessions, createChatSession, archiveChatSession, updateChatSession, respondManagerChatPermission } from './managerChat/ManagerChatManager.js';
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
  RecentLedgerSummary,
  DependencyBlocker
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
  
  broadcast(message: ServerMessage, exclude?: WebSocket, profile?: string) {
    const messageStr = JSON.stringify(message);
    for (const [ws, info] of this.sessions) {
      if (ws !== exclude && ws.readyState === WebSocket.OPEN) {
        // Profile-scoped chat pushes only reach clients subscribed to that
        // profile; everything else still fans out to all connected clients.
        if (profile && info.profile !== profile) continue;
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
  registryService: new RegistryService(undefined, getCoordinatorIdentity().advertised_url),
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
  
  // Set up push bus to broadcast to all connected clients. Chat messages are
  // scoped to the profile they concern; everything else fans out everywhere.
  pushBus.subscribe((message: ServerMessage) => {
    if (
      message.type === 'manager.chat.chunk'
      || message.type === 'manager.chat.toolCall'
      || message.type === 'manager.chat.permission'
      || message.type === 'manager.chat.preview'
      || message.type === 'manager.chat.updated'
    ) {
      sessionStore.broadcast(message, undefined, message.profile);
    } else {
      sessionStore.broadcast(message);
    }
  });

  // Live-tee each logged assistant/chunk onto the push bus (#959) so every
  // subscribed client renders a turn progressively, not just the sender.
  setChunkPublisher((chunk) => pushBus.publish(chunk));
  // Slice 3: structured tool-call activity and permission requests ride the
  // same profile-scoped broadcast as chunks.
  setChatEventPublishers({
    toolCall: (event) => pushBus.publish(event),
    permission: (event) => pushBus.publish(event),
    updated: (event) => pushBus.publish(event)
  });
  // WP3: preview-port detection pushes the same way.
  setPreviewPublisher((event) => pushBus.publish(event));
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
      
    case 'manager.chat.send':
      await handleManagerChatSend(ws, message, requestId);
      break;

    case 'manager.chat.steer':
      await handleManagerChatSteer(ws, message, requestId);
      break;

    case 'manager.chat.cancel':
      await handleManagerChatCancel(ws, message, requestId);
      break;

    case 'manager.chat.historyRequest':
      await handleManagerChatHistoryRequest(ws, message, requestId);
      break;

    case 'manager.chat.permission.respond':
      await handleManagerChatPermissionRespond(ws, message, requestId);
      break;

    case 'manager.chat.sessionList':
      await handleManagerChatSessionList(ws, message, requestId);
      break;

    case 'manager.chat.sessionCreate':
      await handleManagerChatSessionCreate(ws, message, requestId);
      break;

    case 'manager.chat.sessionUpdate':
      await handleManagerChatSessionUpdate(ws, message, requestId);
      break;

    case 'manager.chat.sessionArchive':
      await handleManagerChatSessionArchive(ws, message, requestId);
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

async function handleManagerChatSend(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.send' }>, requestId: string) {
  try {
    const { turn, cancelled } = await sendManagerChatMessage(message.profile, message.message, requestId, message.sessionId);

    // Send a reply for both outcomes: a cancelled turn reports its partial
    // text plus `cancelled: true` so the client can resolve its in-flight
    // busy state deterministically (the updated→history refetch below then
    // shows the durable turn). #1001 — previously a cancelled turn sent no
    // reply, so a client whose pending request had been resynced mid-turn
    // dropped the updated push and froze with turnBusy stuck true.
    const payload: ServerMessage = {
      type: 'manager.chat.reply',
      requestId,
      profile: message.profile,
      ...(message.sessionId ? { sessionId: message.sessionId } : {}),
      reply: turn.text,
      backend: turn.backend!,
      model: turn.model ?? null,
      usage: turn.usage,
      cancelled: cancelled || undefined
    };
    ws.send(JSON.stringify(payload));
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  } finally {
    pushBus.publish({
      type: 'manager.chat.updated',
      profile: message.profile,
      ...(message.sessionId ? { sessionId: message.sessionId } : {}),
      requestId
    });
  }
}

async function handleManagerChatCancel(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.cancel' }>, requestId: string) {
  try {
    const cancelled = await cancelManagerChatTurn(message.profile, message.sessionId);
    if (!cancelled) {
      ws.send(JSON.stringify(createErrorResponse(requestId, new Error('No turn is in flight for this profile.'))));
    }
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleManagerChatSteer(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.steer' }>, requestId: string) {
  try {
    const result = await steerManagerChatTurn(message.profile, message.message, message.sessionId);
    ws.send(JSON.stringify({
      type: 'manager.chat.steered',
      requestId,
      profile: message.profile,
      ...(message.sessionId ? { sessionId: message.sessionId } : {}),
      outcome: result.outcome
    } satisfies ServerMessage));
    pushBus.publish({
      type: 'manager.chat.updated',
      requestId,
      profile: message.profile,
      ...(message.sessionId ? { sessionId: message.sessionId } : {})
    });
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleManagerChatHistoryRequest(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.historyRequest' }>, requestId: string) {
  const view = getManagerChatSessionView(message.profile, message.sessionId);
  const payload: ServerMessage = {
    type: 'manager.chat.history',
    requestId,
    profile: message.profile,
    ...(message.sessionId ? { sessionId: message.sessionId } : {}),
    turns: view.turns,
    cursor: view.cursor,
    streaming: view.streaming,
    permission: view.permission
  };
  ws.send(JSON.stringify(payload));
}

async function handleManagerChatPermissionRespond(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.permission.respond' }>, requestId: string) {
  try {
    const answered = await respondManagerChatPermission(message.profile, message.sessionId, message.permissionId, message.optionId);
    if (!answered) {
      ws.send(JSON.stringify(createErrorResponse(requestId, new Error('No pending permission request for this conversation.'))));
    }
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleManagerChatSessionList(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.sessionList' }>, requestId: string) {
  const payload: ServerMessage = {
    type: 'manager.chat.sessionList',
    requestId,
    profile: message.profile,
    sessions: listChatSessions(message.profile)
  };
  ws.send(JSON.stringify(payload));
}

async function handleManagerChatSessionCreate(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.sessionCreate' }>, requestId: string) {
  try {
    const session = await createChatSession(message.profile, message.backend, message.model ?? null, message.title, message.reasoningEffort);
    const payload: ServerMessage = {
      type: 'manager.chat.sessionCreated',
      requestId,
      profile: message.profile,
      session
    };
    ws.send(JSON.stringify(payload));
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleManagerChatSessionUpdate(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.sessionUpdate' }>, requestId: string) {
  try {
    const patch: { backend?: string; model?: string | null; reasoningEffort?: string | null; title?: string } = {};
    if (message.backend !== undefined) patch.backend = message.backend;
    if (message.model !== undefined) patch.model = message.model;
    if (message.reasoningEffort !== undefined) patch.reasoningEffort = message.reasoningEffort;
    if (message.title !== undefined) patch.title = message.title;
    const session = updateChatSession(message.profile, message.sessionId, patch);
    const payload: ServerMessage = {
      type: 'manager.chat.sessionUpdated',
      requestId,
      profile: message.profile,
      session
    };
    ws.send(JSON.stringify(payload));
  } catch (error) {
    ws.send(JSON.stringify(createErrorResponse(requestId, error instanceof Error ? error : new Error(String(error)))));
  }
}

async function handleManagerChatSessionArchive(ws: WebSocket, message: Extract<ClientMessage, { type: 'manager.chat.sessionArchive' }>, requestId: string) {
  try {
    const session = await archiveChatSession(message.profile, message.sessionId);
    const payload: ServerMessage = {
      type: 'manager.chat.sessionArchived',
      requestId,
      profile: message.profile,
      session
    };
    ws.send(JSON.stringify(payload));
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
    let dependencyBlockers: DependencyBlocker[] = [];
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
      dependencyBlockers = status.dependency_blockers ?? [];
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
      dependencyBlockers?: DependencyBlocker[];
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
      dependencyBlockers,
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

// Exposes the live fleetDispatch coordinator (reassigned by
// createWebSocketHandler once real deps are available) so HTTP routes
// (POST /api/dispatch) can submit sessions through the same coordinator
// that already streams their progress over the WebSocket push bus --
// no separate dispatch-execution path to keep in sync.
export function getFleetDispatch() {
  return fleetDispatch;
}

export { WebSocketSessionStore, sessionStore };
