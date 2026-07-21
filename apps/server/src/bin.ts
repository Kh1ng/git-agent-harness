#!/usr/bin/env node

import { createServer as createExpressServer } from './server.js';
import { createServer as createHttpServer } from 'http';
import { WebSocketServer } from 'ws';
import { createWebSocketHandler } from './wsServer.js';
import { isGahCliAvailable } from './gahCli.js';
import { getProviderRegistry } from './provider/ProviderRegistry.js';
import { markReadinessCheck } from './serverReadiness.js';

const PORT = parseInt(process.env.PORT || '3773');
const HOST = process.env.HOST || '0.0.0.0';

async function main() {
  console.log('Starting Git Agent Harness server...');
  
  // Create Express app
  const app = createExpressServer({ coordinatorPort: PORT });
  
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
