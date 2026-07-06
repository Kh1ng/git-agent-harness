#!/usr/bin/env node

import { createServer as createExpressServer } from './server.js';
import { createServer as createHttpServer } from 'http';
import { WebSocketServer } from 'ws';
import { createWebSocketHandler } from './wsServer.js';
import { startRustBackendProxy } from './rustBackend.js';
import { isGahCliAvailable } from './gahCli.js';
import { getProviderRegistry } from './provider/ProviderRegistry.js';

const PORT = parseInt(process.env.PORT || '3773');

async function main() {
  console.log('Starting Git Agent Harness server...');
  
  // Create Express app
  const app = createExpressServer();
  
  // Create HTTP server from Express app
  const server = createHttpServer(app);
  
  // Create WebSocket server
  const wss = new WebSocketServer({ server });
  
  // Check GAH CLI availability and start legacy Rust backend proxy
  const cliAvailable = await isGahCliAvailable();
  if (cliAvailable) {
    console.log('GAH CLI is available - using real CLI integration');
  } else {
    console.log('GAH CLI not found - running in limited mode');
    // Start legacy Rust backend proxy as fallback
    await startRustBackendProxy();
  }
  
  // Initialize provider registry with default profile
  const providerRegistry = getProviderRegistry();
  providerRegistry.setDefaultProfile('gah');
  
  // Try to load real status data if CLI is available
  if (cliAvailable) {
    try {
      await providerRegistry.loadFromGahCli('gah');
      console.log('Loaded real provider statuses from GAH CLI');
    } catch (error) {
      console.warn('Failed to load provider statuses from GAH CLI:', error);
    }
  }
  
  // Set up WebSocket handler
  createWebSocketHandler(wss);
  
  // Start HTTP server
  server.listen(PORT, () => {
    console.log(`Git Agent Harness server listening on port ${PORT}`);
    console.log(`WebSocket server available at ws://localhost:${PORT}`);
    console.log(`Health check at http://localhost:${PORT}/health`);
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