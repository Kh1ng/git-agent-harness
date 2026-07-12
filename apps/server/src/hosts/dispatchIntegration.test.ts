/**
 * Integration tests for dispatch-to-host routing and REST/WS integration
 */

import assert from 'assert';
import express from 'express';
import { createServer } from '../server.js';
import { getSessionManager } from '../sessions/SessionManager.js';
import { getHostRegistry } from './HostRegistry.js';
import { WebSocket } from 'ws';
import type { ServerMessage, ClientMessage, Session } from '@git-agent-harness/contracts';

console.log('Running dispatch integration tests...');

async function runTests() {
  const hostRegistry = getHostRegistry();
  const sessionManager = getSessionManager();

  // Mock SessionManager.startSession to avoid invoking real gah CLI
  const originalStartSession = sessionManager.startSession;
  let sessionCounter = 1;
  sessionManager.startSession = async (options) => {
    const session = {
      id: `mock-session-${sessionCounter++}`,
      providerKind: options.providerKind,
      instanceId: options.instanceId,
      status: 'running' as const,
      startedAt: new Date().toISOString(),
      repo: options.repo,
      mode: options.mode,
      hostId: options.hostId || 'local'
    };
    (sessionManager as any).sessions.set(session.id, session);
    return session;
  };

  // 1. Start a mock remote server listening on 13774
  const mockRemoteApp = express();
  mockRemoteApp.use(express.json());
  mockRemoteApp.post('/api/dispatch', async (req, res) => {
    try {
      const session = await sessionManager.startSession({
        ...req.body,
        // Ensure hostId is set to remote1 since it runs here
        hostId: 'remote1'
      });
      res.status(201).json(session);
    } catch (error) {
      res.status(500).json({ error: String(error) });
    }
  });

  const remoteServer = mockRemoteApp.listen(0);
  const remotePort = (remoteServer.address() as any).port;
  console.log(`Mock remote server listening on ${remotePort}`);

  // Register the remote host in HostRegistry
  hostRegistry.clear();
  hostRegistry.setHost({
    id: 'remote1',
    base_url: `http://localhost:${remotePort}`,
    profile: 'gah'
  });

  // 2. Start the local server listening on 0
  const localApp = createServer();
  const localServer = localApp.listen(0);
  const localPort = (localServer.address() as any).port;
  console.log(`Local server listening on ${localPort}`);

  // Let's create a WebSocket server on the local server so we can test WS startSession
  const { WebSocketServer } = await import('ws');
  const wss = new WebSocketServer({ noServer: true });
  localServer.on('upgrade', (request, socket, head) => {
    // Basic upgrade handler for test
    if (request.url === '/ws') {
      wss.handleUpgrade(request, socket, head, (ws) => {
        wss.emit('connection', ws, request);
      });
    } else {
      socket.destroy();
    }
  });

  // Import handleStartSession and pushBus to route messages
  const { handleStartSession } = await import('../wsServer.js');

  wss.on('connection', (ws) => {
    ws.on('message', async (data) => {
      try {
        const message = JSON.parse(data.toString()) as ClientMessage;
        if (message.type === 'session.start') {
          await handleStartSession(ws as any, message, message.requestId);
        }
      } catch (err) {
        console.error('WS Connection error:', err);
      }
    });
  });

  try {
    // Test API 1: Direct POST /api/dispatch to local host runs locally
    console.log('Testing Direct POST /api/dispatch...');
    const postResponse = await fetch(`http://localhost:${localPort}/api/dispatch`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        profile: 'gah',
        providerKind: 'codex',
        instanceId: 'codex_instance',
        repo: 'local-repo',
        mode: 'dispatch',
        hostId: 'local'
      })
    });

    assert.strictEqual(postResponse.status, 201);
    const postSession = await postResponse.json() as Session;
    assert.strictEqual(postSession.hostId, 'local');
    assert.strictEqual(postSession.status, 'running');
    console.log('✅ Direct POST test passed');

    // Test API 2: WebSocket session.start routed to remote host
    console.log('Testing WebSocket session.start routed to remote host...');
    await new Promise<void>((resolve, reject) => {
      const client = new WebSocket(`ws://localhost:${localPort}/ws`);
      
      client.on('open', () => {
        client.send(JSON.stringify({
          type: 'session.start',
          requestId: 'req-456',
          profile: 'gah',
          providerKind: 'codex',
          instanceId: 'codex_instance',
          repo: 'remote-repo',
          mode: 'dispatch',
          hostId: 'remote1' // Pinned to remote1
        }));
      });

      client.on('message', (data) => {
        try {
          const response = JSON.parse(data.toString()) as ServerMessage;
          if (response.type === 'session.started') {
            assert.strictEqual(response.session.hostId, 'remote1', 'Session should run on remote1');
            assert.strictEqual(response.session.repo, 'remote-repo');
            console.log('✅ WebSocket remote routing test passed');
            client.close();
            resolve();
          } else if (response.type === 'error') {
            reject(new Error(response.error));
          }
        } catch (err) {
          reject(err);
        }
      });

      client.on('error', reject);
    });

    // Test API 3: WebSocket session.start automatic routing using least_loaded strategy
    console.log('Testing WebSocket session.start automatic load routing...');
    // Currently, local has 1 active session (from postSession)
    // remote1 has 1 active session (from the WebSocket remote routing test)
    // Let's add another session to local to make it 2, and remote1 has 1.
    // Let's add a second remote host `remote2` with 0 sessions
    const offlinePort = localPort + 100;
    hostRegistry.setHost({
      id: 'remote2',
      base_url: `http://localhost:${offlinePort}`, // mock / unused for now
      profile: 'gah'
    });

    // Mock chooseHost to see if it selects remote2 because it has 0 load
    // local has 1 (from postSession, since ws tests are mock/in-memory and share the sessionManager)
    // remote1 has 1
    // remote2 has 0
    await new Promise<void>((resolve, reject) => {
      const client = new WebSocket(`ws://localhost:${localPort}/ws`);
      
      client.on('open', () => {
        client.send(JSON.stringify({
          type: 'session.start',
          requestId: 'req-789',
          profile: 'gah',
          providerKind: 'codex',
          instanceId: 'codex_instance',
          repo: 'auto-repo',
          mode: 'dispatch',
          routingStrategy: 'least_loaded' // routingStrategy is specified, no hostId
        }));
      });

      client.on('message', async (data) => {
        try {
          const response = JSON.parse(data.toString()) as ServerMessage;
          if (response.type === 'session.started') {
            console.log('Received session.started in automatic load test:', response.session);
            reject(new Error('Should not have succeeded without routing or fail handling'));
          } else if (response.type === 'error') {
            console.log('Received error response:', response.error);
            assert.ok(response.error.includes(String(offlinePort)) || response.error.includes('ECONNREFUSED') || response.error.includes('fetch failed'), 'Should fail connecting to remote2');
            console.log('✅ WebSocket automatic load routing test passed (successfully routed to remote2 and failed as expected)');
            client.close();
            resolve();
          }
        } catch (err) {
          reject(err);
        }
      });

      client.on('error', reject);
    });

  } finally {
    // Clean up
    remoteServer.close();
    localServer.close();
    wss.close();
    sessionManager.startSession = originalStartSession;
    hostRegistry.clear();
  }

  console.log('All integration tests passed successfully!');
}

runTests().catch((err) => {
  console.error('Integration tests failed:', err);
  process.exit(1);
});
