import express from 'express';
import cors from 'cors';
import { getServerReadiness } from './serverReadiness.js';
import {
  runStatus,
  runQuota,
  runReport,
  runReportSeries,
  runLedgerWork,
  runEvents,
  runProfileList,
  runProfileAdd,
  runProfileSet,
  runProfileRemove,
  runConfigSet,
  runConfigShow,
  runConfigShowProfile,
  runDoctor,
  getLoopStatus,
  startLoop,
  stopLoop,
  type ProfileAddOptions,
  type ProfileSetOptions,
  type ProfileRemoveOptions,
  type ConfigSetOptions
} from './gahCli.js';
import type {
  ReportGroupBy,
  ReportSeriesData,
  ConfigProfileSummary,
  DoctorSnapshot
} from '@git-agent-harness/contracts';
import { deriveControllerActivity } from './controllerActivity.js';
import { authMiddleware } from './authMiddleware.js';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { RegistryService } from './registryService.js';

const SERVER_VERSION = '0.1.0';

type ConfigEffectiveDeps = {
  runConfigShowProfile: (profile: string) => Promise<ConfigProfileSummary>;
  runDoctor: (profile: string) => Promise<DoctorSnapshot>;
};

type CreateServerOptions = Partial<ConfigEffectiveDeps> & {
  registryService?: RegistryService;
  coordinatorPort?: number;
};

const DEFAULT_CONFIG_EFFECTIVE_DEPS: ConfigEffectiveDeps = {
  runConfigShowProfile,
  runDoctor
};

/** Same hardcoded default as wsServer.ts's welcome message, until Settings
 * gains real profile switching (see apps/web Settings page). */
const DEFAULT_PROFILE = 'gah';

export function createServer(
  configDeps: CreateServerOptions = {}
): express.Express {
  const configEffectiveDeps: ConfigEffectiveDeps = {
    ...DEFAULT_CONFIG_EFFECTIVE_DEPS,
    ...configDeps
  };
  const coordinatorPort = configDeps.coordinatorPort ?? 3773;

  const registryService = configDeps.registryService || new RegistryService();

  const app = express();

  // Middleware
  app.use(cors());
  app.use(express.json());
  app.use(authMiddleware);

  // Health check endpoint
  app.get('/health', (req, res) => {
    const readiness = getServerReadiness();
    const status = readiness.isReady ? 'healthy' : 'starting';
    const identity = getCoordinatorIdentity(undefined, coordinatorPort);

    res.json({
      status,
      node_id: identity.node_id,
      display_name: identity.display_name,
      advertised_url: identity.advertised_url,
      version: identity.version,
      schema_digest: identity.schema_digest,
      timestamp: Date.now(),
      checks: readiness.checks
    });
  });

  // API info endpoint
  app.get('/api/info', (req, res) => {
    const identity = getCoordinatorIdentity(undefined, coordinatorPort);
    res.json({
      name: 'Git Agent Harness',
      version: SERVER_VERSION,
      description: 'A WebSocket server for managing Git Agent Harness sessions and providers',
      identity: {
        node_id: identity.node_id,
        display_name: identity.display_name,
        advertised_url: identity.advertised_url,
        version: identity.version,
        schema_digest: identity.schema_digest
      },
      endpoints: {
        health: '/health',
        info: '/api/info',
        status: '/api/status',
        quota: '/api/quota',
        doctor: '/api/doctor',
        report: '/api/report',
        work: '/api/work/:workId',
        events: '/api/events',
        controllerActivity: '/api/controller-activity',
        profiles: '/api/profiles',
        config: '/api/config',
        configEffective: '/api/config/effective',
        loopStatus: '/api/loop/status',
        loopStart: '/api/loop/start',
        loopStop: '/api/loop/stop',
        websocket: '/ws',
        registryNodes: '/api/registry/nodes',
        registryNodeHealth: '/api/registry/nodes/:nodeId/health',
        registryNodeRotateSecret: '/api/registry/nodes/:nodeId/rotate-secret'
      },
      features: {
        webSocket: true,
        providerManagement: true,
        sessionManagement: true,
        rustBackendProxy: true,
        nodeRegistry: true
      }
    });
  });

  // Registry API endpoints
  app.get('/api/registry/nodes', (req, res) => {
    try {
      res.json(registryService.getNodesSummary());
    } catch (error) {
      res.status(500).json({
        error: 'Internal Server Error',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/registry/nodes', (req, res) => {
    try {
      const { warnings } = registryService.registerNode(req.body);
      res.status(201).json({
        success: true,
        message: 'Node registered successfully',
        warnings
      });
    } catch (error) {
      res.status(400).json({
        error: 'Bad Request',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.delete('/api/registry/nodes/:nodeId', (req, res) => {
    try {
      const revoked = registryService.revokeNode(req.params.nodeId);
      if (!revoked) {
        res.status(404).json({
          error: 'Not Found',
          message: `Node ${req.params.nodeId} not found`
        });
        return;
      }
      res.json({
        success: true,
        message: 'Node registration revoked successfully'
      });
    } catch (error) {
      res.status(500).json({
        error: 'Internal Server Error',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/registry/nodes/:nodeId/health', async (req, res) => {
    try {
      const health = await registryService.checkNodeHealth(req.params.nodeId);
      res.json(health);
    } catch (error) {
      res.status(404).json({
        error: 'Not Found',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/registry/nodes/:nodeId/rotate-secret', (req, res) => {
    try {
      const { secret_ref } = req.body;
      registryService.rotateSecret(req.params.nodeId, secret_ref);
      res.json({
        success: true,
        message: 'Secret rotated successfully'
      });
    } catch (error) {
      res.status(400).json({
        error: 'Bad Request',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Pull-data REST endpoints (TICKET-productization): these are on-demand
  // fetches -- report parameters, one work item's timeline -- that don't
  // fit the WS welcome message's connect-once push shape. Live/push data
  // (sessions, provider status) stays on the WebSocket; this is
  // additive, it does not replace or narrow the existing WS contract.
  app.get('/api/status', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      const status = await runStatus(profile);
      res.json(status);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah status',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/quota', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const since = typeof req.query.since === 'string' ? req.query.since : '7d';
    try {
      const quota = await runQuota({ profile, since });
      res.json(quota);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah quota snapshot',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/doctor', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      res.json(await configEffectiveDeps.runDoctor(profile));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to run gah doctor',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/report', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : undefined;
    const since = typeof req.query.since === 'string' ? req.query.since : undefined;
    const groupByRaw = typeof req.query.groupBy === 'string' ? req.query.groupBy : undefined;
    const groupBy: ReportGroupBy | undefined =
      groupByRaw === 'model' || groupByRaw === 'backend' ? groupByRaw : undefined;
    try {
      const report = await runReport({ profile, since, groupBy });
      res.json(report);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah report',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/report/series', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : undefined;
    const since = typeof req.query.since === 'string' ? req.query.since : undefined;
    const bucket = typeof req.query.bucket === 'string' ? req.query.bucket : undefined;
    try {
      const series: ReportSeriesData = await runReportSeries({ profile, since, bucket });
      res.json(series);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah report series',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/work/:workId', async (req, res) => {
    try {
      const entries = await runLedgerWork(req.params.workId);
      res.json(entries);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load work item history',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Config-driven profile discovery: lets the frontend list/switch between
  // real configured repos instead of a blind free-text profile field. See
  // apps/web SettingsPage.
  app.get('/api/profiles', async (req, res) => {
    try {
      const profiles = await runProfileList();
      res.json(profiles);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah profiles',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Profile CRUD operations for Issue #148
  app.post('/api/profiles', async (req, res) => {
    try {
      const options: ProfileAddOptions = {
        ...req.body,
        // Ensure required fields are present
        name: req.body.name,
        display_name: req.body.display_name,
        repo_id: req.body.repo_id,
        provider: req.body.provider,
        repo: req.body.repo,
        local_path: req.body.local_path,
        artifact_root: req.body.artifact_root,
      };
      await runProfileAdd(options);
      res.status(201).json({ success: true, message: `Profile '${req.body.name}' added` });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to add profile',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.patch('/api/profiles/:name', async (req, res) => {
    try {
      const options: ProfileSetOptions = {
        name: req.params.name,
        ...req.body,
      };
      await runProfileSet(options);
      res.json({ success: true, message: `Profile '${req.params.name}' updated` });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to update profile',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.delete('/api/profiles/:name', async (req, res) => {
    try {
      const options: ProfileRemoveOptions = {
        name: req.params.name,
        force: req.query.force === 'true',
      };
      await runProfileRemove(options);
      res.json({ success: true, message: `Profile '${req.params.name}' removed` });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to remove profile',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Global config defaults (current_manager, etc.) -- Issue #194. Read-only
  // GET plus a PATCH/POST that shells out to `gah config set` so the TOML
  // config stays the single source of truth and the running loop picks the
  // change up on its next iteration without a restart.
  app.get('/api/config', async (_req, res) => {
    try {
      const config = await runConfigShow();
      res.json(config);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to read global config',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/config', async (req, res) => {
    try {
      const options: ConfigSetOptions = {
        current_manager: req.body.current_manager,
        clear: req.body.clear,
      };
      await runConfigSet(options);
      res.json({ success: true, message: 'Global config updated' });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to update global config',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/config/effective', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      const config = await configEffectiveDeps.runConfigShowProfile(profile);
      res.json(config);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load effective config',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Start/stop/status for the `gah loop --profile <p>` daemon, so a stuck
  // loop can be killed from the dashboard instead of requiring SSH/terminal
  // access. Conflict detection is `gah`'s own per-profile flock
  // (acquire_profile_lock in src/controller.rs) -- see gahCli.ts for why the
  // check isn't reimplemented here.
  app.get('/api/loop/status', (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    res.json(getLoopStatus(profile));
  });

  app.post('/api/loop/start', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    try {
      const result = await startLoop(profile);
      if (!result.started) {
        res.status(409).json(result);
        return;
      }
      res.json(result);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to start gah loop',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/loop/stop', (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const result = stopLoop(profile);
    if (!result.stopped) {
      res.status(409).json(result);
      return;
    }
    res.json(result);
  });

  app.get('/api/events', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const since = typeof req.query.since === 'string' ? req.query.since : '7d';
    try {
      const events = await runEvents(profile, since);
      res.json(events);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah events',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/controller-activity', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const since = typeof req.query.since === 'string' ? req.query.since : '24h';
    try {
      const events = await runEvents(profile, since);
      res.json(deriveControllerActivity(events));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load controller activity',
        message: error instanceof Error ? error.message : String(error)
      });
    }
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
