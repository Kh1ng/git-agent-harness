#!/usr/bin/env node

import { createServer } from './server.js';
import { WebSocketServer } from 'ws';
import { createWebSocketHandler } from './wsServer.js';
// [TICKET-113] Rust backend proxy no longer needed - using CLI directly

const PORT = parseInt(process.env.PORT || '3773');

async function main() {
  console.log('Starting Git Agent Harness server...');
  console.log('[TICKET-113] Using GAH CLI directly instead of Rust backend proxy');
  
  // Create HTTP server
  const server = createServer();
  
  // Create WebSocket server
  const wss = new WebSocketServer({ server });
  
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