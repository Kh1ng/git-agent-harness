#!/usr/bin/env node

import { createAuthorizedWebSocketServer } from './webSocketAuth.js';
import { startModelPriceRefresh } from './modelPricing.js';
import { helperPriceExtractor } from './modelPriceHelper.js';
import { DeviceAccess } from './deviceAccess.js';
import { createServer as createExpressServer, initializeSkillBank } from './server.js';
import { createServer as createHttpServer } from 'http';
import { createWebSocketHandler } from './wsServer.js';
import { validateNodeRole, workerMemoryEnvironment } from './nodeRole.js';
import { runNodeRole, runProfileList } from './gahCli.js';
import { isGahCliAvailable } from './gahCli.js';
import { getProviderRegistry } from './provider/ProviderRegistry.js';
import { RegistryService } from './registryService.js';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { markReadinessCheck } from './serverReadiness.js';
import {
  InvalidBindHostError,
  resolveBindHost,
  networkExposureWarning,
  validateBindHost
} from './bindHost.js';
import { startChatMaintenanceScheduler, stopChatMaintenanceScheduler } from './managerChat/chatMaintenance.js';
import { ActivityFeed } from './activityFeed.js';
import { WebPushNotifications } from './webPush.js';
import { apnsFromEnvironment } from './apns.js';
import { channelDelivery, commandDelivery, deliverToAll } from './notifyDelivery.js';
import { AuthHealthMonitor, AuthHealthProber, configureChatAuthHealth } from './authHealth.js';
import { LoginRepairBroker, LoginRepairs, loadProviderKeys } from './loginRepair.js';
import { WorkerUpdateBroker } from './workerUpdateBroker.js';
import { WorkerUpdateService } from './workerUpdate.js';
import { createCliRouterQuotaObserver } from './cliRouter.js';
import { startQuotaRefreshScheduler } from './quotaRefreshScheduler.js';

const PORT = parseInt(process.env.PORT || '3773');
const HOST = resolveBindHost();

// launchd and systemd logs carry no timestamps; restart timing is diagnosed
// from these lines.
function logLifecycle(message: string) {
  console.log(`${new Date().toISOString()} ${message}`);
}

async function main() {
  try {
    validateBindHost(HOST);
  } catch (error) {
    if (error instanceof InvalidBindHostError) {
      console.error(`Failed to start server: ${error.message}`);
      process.exit(1);
    }
    throw error;
  }

  logLifecycle('Starting Git Agent Harness server...');
  // Keys repaired from another device (#1272) apply to checks and backends.
  loadProviderKeys();

  const node = validateNodeRole(await runNodeRole());
  Object.assign(process.env, workerMemoryEnvironment(node, process.env.COORDINATOR_TOKEN));
  console.log(`Node role: ${node.role}; central URL: ${node.central_url ?? '(unset)'}`);
  if (node.role === 'central') initializeSkillBank(node);

  const coordinatorIdentity = getCoordinatorIdentity(undefined, PORT);
  const deviceAccess = node.role === 'central' ? new DeviceAccess() : undefined;
  const registryService = new RegistryService(node.role === 'worker' ? null : undefined, coordinatorIdentity.advertised_url, PORT);
  const webPushNotifications = node.role === 'central' ? new WebPushNotifications() : undefined;
  const apnsNotifications = node.role === 'central' ? apnsFromEnvironment() : undefined;
  const activityFeed = new ActivityFeed(undefined, node.role === 'central' ? deliverToAll([
    webPushNotifications && ((event) => webPushNotifications.deliverActivity(event, id => deviceAccess!.canApprovePairing(id))),
    apnsNotifications && ((event) => apnsNotifications.deliverActivity(event, id => deviceAccess!.canApprovePairing(id))),
    channelDelivery(coordinatorIdentity.advertised_url),
    commandDelivery(coordinatorIdentity.advertised_url)
  ]) : undefined);

  // Every node checks its own logins; central watches the fleet's (#1271).
  const authHealthProber = new AuthHealthProber();
  const authHealthMonitor = node.role === 'central'
    ? new AuthHealthMonitor(
      { nodeId: coordinatorIdentity.node_id, nodeName: coordinatorIdentity.display_name, prober: authHealthProber },
      () => registryService.getCachedObservations()
    )
    : undefined;
  if (authHealthMonitor) {
    registryService.onChange(() => authHealthMonitor.observationsChanged());
    configureChatAuthHealth(authHealthMonitor);
  }
  // A repair reports success only after this node's login check has re-run.
  const loginRepairs = new LoginRepairs({ nodeId: coordinatorIdentity.node_id, onSuccess: () => authHealthProber.refresh() });
  const loginRepairBroker = node.role === 'central'
    ? new LoginRepairBroker({
      localNodeId: coordinatorIdentity.node_id,
      local: loginRepairs,
      registry: registryService,
      onRemoteSuccess: (nodeId) => { void registryService.checkNodeHealth(nodeId).catch(() => undefined); }
    })
    : undefined;

  const cliRouterQuotaObserver = node.role === 'central' ? createCliRouterQuotaObserver() : undefined;

  // Issue #1416: a worker runs release updates on central's instruction;
  // central brokers the fleet's updates and sweeps opted-in nodes.
  const workerUpdates = node.role === 'worker' ? new WorkerUpdateService() : undefined;
  const workerUpdateBroker = node.role === 'central'
    ? new WorkerUpdateBroker({ localNodeId: coordinatorIdentity.node_id, registry: registryService })
    : undefined;
  let stopWorkerAutoUpdate: (() => void) | undefined;

  // Create Express app
  const app = createExpressServer({
    coordinatorPort: PORT,
    node,
    deviceAccess,
    registryService,
    webPushNotifications,
    apnsNotifications,
    activityFeed,
    authHealthProber,
    authHealthMonitor,
    loginRepairs,
    loginRepairBroker,
    workerUpdates,
    workerUpdateBroker,
    cliRouterQuotaObserver
  });
  
  // Create HTTP server from Express app
  const server = createHttpServer(app);
  
  // Create WebSocket server
  const wss = createAuthorizedWebSocketServer(server, node.role, deviceAccess);
  
  // Check GAH CLI availability (real status/dispatch data is loaded
  // on-demand per WebSocket connection in wsServer.ts's sendWelcomeMessage,
  // not cached at startup).
  const cliAvailable = await isGahCliAvailable();
  if (cliAvailable) {
    console.log('GAH CLI is available - using real CLI integration');
  } else {
    console.log('GAH CLI not found - running in limited mode');
  }
  markReadinessCheck(
    'rustBackend',
    cliAvailable,
    cliAvailable ? undefined : 'gah CLI not found'
  );

  // Initialize provider registry with default profile
  const providerRegistry = getProviderRegistry();
  providerRegistry.setDefaultProfile('gah');
  await providerRegistry.refreshAllFromGah();
  markReadinessCheck('providerRegistry', true);
  
  // Set up WebSocket handler
  createWebSocketHandler(wss, {
    registryService,
    coordinatorIdentity,
    node,
    activityFeed,
    authHealthMonitor,
    // Background delivery must not depend on an open dashboard. Poll every
    // configured profile, but only while some device is registered for push.
    backgroundProfiles: node.role === 'central' && cliAvailable
      ? async () => {
        const devices = (webPushNotifications?.list().count ?? 0) + (apnsNotifications?.list().count ?? 0);
        return devices > 0 ? (await runProfileList()).map((profile) => profile.name) : [];
      }
      : undefined,
    onChatLifecycle: (event) => {
      void apnsNotifications?.deliverChatLifecycle(event).catch((error) => {
        console.error(`[apns] chat lifecycle delivery failed: ${error instanceof Error ? error.message : String(error)}`);
      });
    }
  });
  markReadinessCheck('webSocket', true);
  
  // Node liveness scheduler (issue #883): polls registered nodes on an
  // interval instead of only reacting to dashboard/dispatch activity, so a
  // node with nothing currently dispatching to it still gets flagged if it
  // goes dark. No-op when no nodes are registered.
  if (node.role === 'central') registryService.startLivenessScheduler();

  // Issue #1416: opted-in nodes update themselves whenever central sees
  // them behind; the sweep is quiet when nothing is eligible.
  if (workerUpdateBroker) {
    const timer = setInterval(() => {
      workerUpdateBroker!.autoUpdateSweep().catch((error) => {
        console.error(`Worker auto-update sweep failed: ${error instanceof Error ? error.message : String(error)}`);
      });
    }, 5 * 60_000);
    timer.unref?.();
    stopWorkerAutoUpdate = () => clearInterval(timer);
  }

  // Chat maintenance scheduler (#1036): settles chat sessions whose branch's
  // PR merged/closed (or whose issue closed) on a bounded interval instead
  // of only at the daily prune, so "the work shipped" is visible while it
  // still matters.
  if (node.role === 'central') startChatMaintenanceScheduler();

  let stopRouterQuotaRefresh: (() => void) | undefined;
  let stopQuotaRefresh: (() => Promise<void>) | undefined;
  // Start HTTP server
  server.listen(PORT, HOST, () => {
    logLifecycle(`Git Agent Harness server listening on ${HOST}:${PORT}`);
    // Model prices: checked a few seconds after start when a day old, then daily.
    if (process.env.GAH_DISABLE_PRICE_REFRESH !== '1') startModelPriceRefresh(helperPriceExtractor('gah'), logLifecycle);
    console.log(`WebSocket server available on ws://${HOST}:${PORT}`);
    console.log(`Health check available on http://${HOST}:${PORT}/health`);
    const warning = networkExposureWarning(HOST);
    if (warning) {
      console.warn(warning);
    }
    // After listening: login checks spawn provider CLIs and must never delay health.
    if (cliAvailable) {
      authHealthProber.start();
      stopQuotaRefresh = startQuotaRefreshScheduler();
      stopRouterQuotaRefresh = cliRouterQuotaObserver?.start();
    }
  });
  
  // Close listeners immediately and terminate the quota process tree before
  // exit; keep-alive or WebSocket connections do not hold up shutdown.
  let shuttingDown = false;
  const shutdown = async () => {
    if (shuttingDown) return;
    shuttingDown = true;
    logLifecycle('Shutting down...');
    registryService.stopLivenessScheduler();
    stopChatMaintenanceScheduler();
    stopWorkerAutoUpdate?.();
    authHealthProber.stop();
    server.close();
    stopRouterQuotaRefresh?.();
    await stopQuotaRefresh?.();
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}

main().catch((error) => {
  console.error('Failed to start server:', error);
  process.exit(1);
});
