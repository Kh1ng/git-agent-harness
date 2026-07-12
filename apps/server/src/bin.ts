#!/usr/bin/env node

import { createServer as createExpressServer } from './server.js';
import { createServer as createHttpServer } from 'http';
import { WebSocketServer } from 'ws';
import { createWebSocketHandler } from './wsServer.js';
import { isGahCliAvailable } from './gahCli.js';
import { getProviderRegistry } from './provider/ProviderRegistry.js';
import { markReadinessCheck } from './serverReadiness.js';
import { createServerPushBus } from './serverPushBus.js';
import { getStatusAggregator, createHostsStatusMessage } from './hosts/statusAggregator.js';
import { getHostRegistry } from './hosts/HostRegistry.js';

const PORT = parseInt(process.env.PORT || '3773');
const HOST = process.env.HOST || '0.0.0.0';

/**
 * MS-2: Set up periodic hosts status refresh
 * Fetches status from all configured hosts every 30 seconds and broadcasts updates
 */
function setupHostsStatusRefresh(pushBus: any) {
  const REFRESH_INTERVAL_MS = 30000; // 30 seconds
  
  async function refreshAndBroadcast() {
    try {
      const statusAggregator = getStatusAggregator();
      const hostsStatus = await statusAggregator.refreshAll();
      
      // Get the updated merged status
      const mergedStatus = await statusAggregator.getMergedStatus();
      
      // Broadcast to all connected clients
      const message = createHostsStatusMessage(mergedStatus);
      pushBus.publish(message);
      
      console.log(`Broadcasted hosts status update to ${pushBus.subscriberCount} clients`);
    } catch (error) {
      console.error('Failed to refresh hosts status:', error);
    }
  }
  
  // Initial refresh
  refreshAndBroadcast();
  
  // Set up periodic refresh
  const intervalId = setInterval(refreshAndBroadcast, REFRESH_INTERVAL_MS);
  
  // Clean up on shutdown
  process.on('SIGINT', () => clearInterval(intervalId));
  process.on('SIGTERM', () => clearInterval(intervalId));
  
  return intervalId;
}

async function main() {
  console.log('Starting Git Agent Harness server...');
  
  // Create Express app
  const app = createExpressServer();
  
  // Create HTTP server from Express app
  const server = createHttpServer(app);
  
  // Create WebSocket server
  const wss = new WebSocketServer({ server });
  
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
  createWebSocketHandler(wss);
  markReadinessCheck('webSocket', true);
  
  // Create push bus for broadcasting to all clients
  const pushBus = createServerPushBus();
  
  // MS-2: Set up periodic hosts status refresh
  setupHostsStatusRefresh(pushBus);
  
  // Start HTTP server
  server.listen(PORT, HOST, () => {
    console.log(`Git Agent Harness server listening on ${HOST}:${PORT}`);
    console.log(`WebSocket server available on ws://${HOST}:${PORT}`);
    console.log(`Health check available on http://${HOST}:${PORT}/health`);
  });
  
  // Handle graceful shutdown
  process.on('SIGINT', () => {
    console.log('Shutting down...');
    server.close();
    process.exit(0);
  });
  
  process.on('SIGTERM', () => {
    console.log('Shutting down...');
    server.close();
    process.exit(0);
  });
}

main().catch((error) => {
  console.error('Failed to start server:', error);
  process.exit(1);
});
