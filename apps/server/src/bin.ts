#!/usr/bin/env node

import { createServer as createExpressServer } from './server.js';
import { createServer as createHttpServer } from 'http';
import { WebSocketServer } from 'ws';
import { createWebSocketHandler } from './wsServer.js';
import { startRustBackendProxy } from './rustBackend.js';

const PORT = parseInt(process.env.PORT || '3773');

async function main() {
  console.log('Starting Git Agent Harness server...');
  
  // Create Express app
  const app = createExpressServer();
  
  // Create HTTP server from Express app
  const server = createHttpServer(app);
  
  // Create WebSocket server
  const wss = new WebSocketServer({ server });
  
  // Start Rust backend proxy
  await startRustBackendProxy();
  
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