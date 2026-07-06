import express from 'express';
import cors from 'cors';
import { getServerReadiness } from './serverReadiness.js';

const SERVER_VERSION = '0.1.0';

export function createServer() {
  const app = express();
  
  // Middleware
  app.use(cors());
  app.use(express.json());
  
  // Health check endpoint
  app.get('/health', (req, res) => {
    const readiness = getServerReadiness();
    const status = readiness.isReady ? 'healthy' : 'starting';
    
    res.json({
      status,
      version: SERVER_VERSION,
      timestamp: Date.now(),
      checks: readiness.checks
    });
  });
  
  // API info endpoint
  app.get('/api/info', (req, res) => {
    res.json({
      name: 'Git Agent Harness',
      version: SERVER_VERSION,
      description: 'A WebSocket server for managing Git Agent Harness sessions and providers',
      endpoints: {
        health: '/health',
        info: '/api/info',
        websocket: 'ws://localhost:3773'
      },
      features: {
        webSocket: true,
        providerManagement: true,
        sessionManagement: true,
        rustBackendProxy: false, // [TICKET-113] Disabled - using CLI directly
        gahCliProxy: true
      }
    });
  });
  
  // 404 handler
  app.use((req, res) => {
    res.status(404).json({
      error: 'Not Found',
      message: `Route ${req.method} ${req.path} not found`
    });
  });
  
  // Error handler
  app.use((err: Error, req: express.Request, res: express.Response, next: express.NextFunction) => {
    console.error('Server error:', err);
    res.status(500).json({
      error: 'Internal Server Error',
      message: err.message || 'An unexpected error occurred'
    });
  });
  
  return app;
}

export { SERVER_VERSION };