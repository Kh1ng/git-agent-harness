// Issue #883: node liveness scheduler. Real fake HTTP node servers (not
// mocked fetch) and a real notify-command subprocess (writing to a temp
// file, not mocked), matching this repo's existing test convention.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { RegistryService } from './registryService.js';
import { COORDINATOR_SCHEMA_DIGEST } from './coordinatorIdentity.js';

function listen(server: http.Server): Promise<string> {
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as AddressInfo;
      resolve(`http://127.0.0.1:${port}`);
    });
  });
}

function fakeHealthyNodeServer(): http.Server {
  return http.createServer((req, res) => {
    if (req.url?.startsWith('/api/status')) {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ version: '0.1.0', schema_digest: COORDINATOR_SCHEMA_DIGEST, generated_at: new Date().toISOString() }));
      return;
    }
    res.writeHead(404);
    res.end();
  });
}

function tempRegistryPath(): string {
  const dir = mkdtempSync(join(tmpdir(), 'gah-registry-test-'));
  return join(dir, 'registry-config.json');
}

test('a node that never answers crosses the bad-check threshold and alerts exactly once', async () => {
  const service = new RegistryService(tempRegistryPath());
  // Port 1 is a real, always-closed privileged port -- fetch fails fast
  // with ECONNREFUSED instead of hanging for the observation timeout.
  service.registerNode({
    node_id: 'dead-node',
    display_name: 'Dead Node',
    advertised_url: 'http://127.0.0.1:1',
    version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'loopback',
    secret_ref: 'env:UNUSED'
  });

  const alertLog = join(mkdtempSync(join(tmpdir(), 'gah-alert-test-')), 'alerts.log');
  const originalCommand = process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND;
  process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND = `cat >> ${alertLog}`;
  try {
    // Below threshold (2 checks): no alert yet.
    await service.runLivenessCheck();
    await service.runLivenessCheck();
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(existsSync(alertLog), false, 'must not alert before crossing the threshold');

    // Crosses the threshold (3rd consecutive bad check).
    await service.runLivenessCheck();
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(existsSync(alertLog), true, 'must alert once the threshold is crossed');
    const firstAlertContent = readFileSync(alertLog, 'utf8');
    assert.match(firstAlertContent, /Dead Node/);
    assert.match(firstAlertContent, /dead-node/);

    // A 4th consecutive bad check must NOT alert again (no spam).
    writeFileSync(alertLog, '');
    await service.runLivenessCheck();
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(readFileSync(alertLog, 'utf8'), '', 'must not re-alert on every subsequent bad check');
  } finally {
    process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND = originalCommand;
  }
});

test('a normally-responsive node never alerts across many checks', async () => {
  const server = fakeHealthyNodeServer();
  const advertisedUrl = await listen(server);
  const service = new RegistryService(tempRegistryPath());
  service.registerNode({
    node_id: 'healthy-node',
    display_name: 'Healthy Node',
    advertised_url: advertisedUrl,
    version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'loopback',
    secret_ref: 'env:UNUSED'
  });

  const alertLog = join(mkdtempSync(join(tmpdir(), 'gah-alert-test-')), 'alerts.log');
  const originalCommand = process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND;
  process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND = `cat >> ${alertLog}`;
  try {
    for (let i = 0; i < 5; i++) {
      await service.runLivenessCheck();
    }
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(existsSync(alertLog), false, 'a healthy node must never trigger an alert');
  } finally {
    process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND = originalCommand;
    server.close();
  }
});

test('recovery resets the counter: a node that goes bad, recovers, then goes bad again alerts again', async () => {
  // A single long-lived server with a toggleable "up" flag, instead of
  // closing/reopening a real socket on the same port (which would race the
  // OS releasing it) -- flip the flag to simulate an outage and a recovery.
  let up = true;
  const server = http.createServer((req, res) => {
    if (!up) {
      req.socket.destroy();
      return;
    }
    if (req.url?.startsWith('/api/status')) {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ version: '0.1.0', schema_digest: COORDINATOR_SCHEMA_DIGEST, generated_at: new Date().toISOString() }));
      return;
    }
    res.writeHead(404);
    res.end();
  });
  const advertisedUrl = await listen(server);

  const service = new RegistryService(tempRegistryPath());
  service.registerNode({
    node_id: 'flappy-node',
    display_name: 'Flappy Node',
    advertised_url: advertisedUrl,
    version: '0.1.0',
    schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'loopback',
    secret_ref: 'env:UNUSED'
  });

  const alertLog = join(mkdtempSync(join(tmpdir(), 'gah-alert-test-')), 'alerts.log');
  const originalCommand = process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND;
  process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND = `cat >> ${alertLog}`;
  try {
    // Go bad: cross the threshold.
    up = false;
    await service.runLivenessCheck();
    await service.runLivenessCheck();
    await service.runLivenessCheck();
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(existsSync(alertLog), true, 'first outage must alert');
    writeFileSync(alertLog, '');

    // Recover: must not immediately re-alert.
    up = true;
    await service.runLivenessCheck();
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(readFileSync(alertLog, 'utf8'), '', 'recovery must not itself alert');

    // Go bad again: must alert again (not permanently silenced).
    up = false;
    await service.runLivenessCheck();
    await service.runLivenessCheck();
    await service.runLivenessCheck();
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(existsSync(alertLog), true, 'a second outage after recovery must alert again');
  } finally {
    process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND = originalCommand;
    server.close();
  }
});
