import express from 'express';
import cors from 'cors';
import { getServerReadiness } from './serverReadiness.js';
import { getSessionManager } from './sessions/SessionManager.js';
import { runStatus, runQuota, runReport, runReportSeries, runLedgerWork, runEvents, runProfileList, runProfileAdd, runProfileSet, runProfileRemove, getLoopStatus, startLoop, stopLoop, type ProfileAddOptions, type ProfileSetOptions, type ProfileRemoveOptions } from './gahCli.js';
import type { ReportGroupBy, ReportSeriesData } from '@git-agent-harness/contracts';
import { deriveControllerActivity } from './controllerActivity.js';

const SERVER_VERSION = '0.1.0';

/** Same hardcoded default as wsServer.ts's welcome message, until Settings
 * gains real profile switching (see apps/web Settings page). */
const DEFAULT_PROFILE = 'gah';

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
        status: '/api/status',
        quota: '/api/quota',
        report: '/api/report',
        work: '/api/work/:workId',
        events: '/api/events',
        controllerActivity: '/api/controller-activity',
        profiles: '/api/profiles',
        loopStatus: '/api/loop/status',
        loopStart: '/api/loop/start',
        loopStop: '/api/loop/stop',
        websocket: '/ws'
      },
      features: {
        webSocket: true,
        providerManagement: true,
        sessionManagement: true,
        rustBackendProxy: true
      }
    });
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

  app.post('/api/dispatch', async (req, res) => {
    try {
      const sessionManager = getSessionManager();
      const session = await sessionManager.startSession({
        profile: req.body.profile,
        providerKind: req.body.providerKind,
        instanceId: req.body.instanceId,
        repo: req.body.repo,
        branch: req.body.branch,
        target: req.body.target,
        mode: req.body.mode,
        backend: req.body.backend,
        model: req.body.model,
        budget: req.body.budget,
        hostId: req.body.hostId
      });
      res.status(201).json(session);
    } catch (error) {
      res.status(500).json({
        error: 'Failed to start remote dispatch session',
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
