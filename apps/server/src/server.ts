import express from 'express';
import cors from 'cors';
import os from 'node:os';
import { statfsSync } from 'node:fs';
import { getServerReadiness } from './serverReadiness.js';
import {
  runStatus,
  runQuota,
  runReport,
  runReportSeries,
  runLedgerWork,
  runSync,
  runLedgerSummary,
  runLedgerClearAttempts,
  runAvailability,
  runAvailabilityClear,
  runHoldSet,
  runHoldClear,
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
import { getFleetDispatch } from './wsServer.js';
import type { SessionOptions } from './sessions/SessionManager.js';
import { deriveControllerActivity } from './controllerActivity.js';
import { authMiddleware } from './authMiddleware.js';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { RegistryService } from './registryService.js';
import { ClaimsService, ClaimConflictError } from './claimsService.js';
import { readSettings as readManagerChatSettings, writeSettings as writeManagerChatSettings } from './managerChat/settingsStore.js';
import { listManagerBackends } from './managerChat/registry.js';
import {
  listCommandsForProfile as listManagerChatCommands,
  listModelsForProfile as listManagerChatModels,
  setModelForProfile as setManagerChatModel
} from './managerChat/ManagerChatManager.js';

const SERVER_VERSION = '0.1.0';

type ConfigEffectiveDeps = {
  runConfigShowProfile: (profile: string) => Promise<ConfigProfileSummary>;
  runDoctor: (profile: string) => Promise<DoctorSnapshot>;
};

type CreateServerOptions = Partial<ConfigEffectiveDeps> & {
  registryService?: RegistryService;
  claimsService?: ClaimsService;
  coordinatorPort?: number;
};

const DEFAULT_CONFIG_EFFECTIVE_DEPS: ConfigEffectiveDeps = {
  runConfigShowProfile,
  runDoctor
};

/** Same hardcoded default as wsServer.ts's welcome message, until Settings
 * gains real profile switching (see apps/web Settings page). */
const DEFAULT_PROFILE = 'gah';

function getLocalResourcePressure() {
  const cpus = os.cpus().length || 0;
  const loadAvg = os.loadavg()[0] || 0;
  const cpuPercent = cpus > 0 ? Math.max(0, Math.min(100, (loadAvg / cpus) * 100)) : null;
  const rssBytes = process.memoryUsage().rss;
  let diskPercent: number | null = null;
  try {
    const stats = statfsSync(process.cwd());
    if (stats.blocks > 0) {
      diskPercent = Math.max(0, Math.min(100, ((stats.blocks - stats.bfree) / stats.blocks) * 100));
    }
  } catch {
    diskPercent = null;
  }
  return {
    cpu_percent: cpuPercent,
    rss_bytes: rssBytes,
    disk_percent: diskPercent
  };
}

export function createServer(
  configDeps: CreateServerOptions = {}
): express.Express {
  const configEffectiveDeps: ConfigEffectiveDeps = {
    ...DEFAULT_CONFIG_EFFECTIVE_DEPS,
    ...configDeps
  };
  const coordinatorPort = configDeps.coordinatorPort ?? 3773;

  const registryService = configDeps.registryService || new RegistryService();
  const claimsService = configDeps.claimsService || new ClaimsService();

  const app = express();
  // Trust X-Forwarded-* only when the immediate hop is loopback (a TLS-terminating
  // reverse proxy on this same host). `true` would trust those headers from any
  // direct peer, letting a remote attacker forge `X-Forwarded-Proto: https` and
  // defeat authMiddleware's TLS requirement outright.
  app.set('trust proxy', 'loopback');

  // Middleware
  app.use(cors());
  app.use(express.json());
  // authMiddleware only guards the node registry -- it is new, narrowly scoped
  // surface. The rest of the API (loop start/stop, config mutation, etc.) is
  // unauthenticated pending #532; applying this globally would silently change
  // that pre-existing contract.
  app.use('/api/registry', authMiddleware);
  app.use('/api/claims', authMiddleware);

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
        fleet: '/api/registry/fleet',
        quota: '/api/quota',
        doctor: '/api/doctor',
        report: '/api/report',
        work: '/api/work/:workId',
        sync: '/api/sync',
        ledgerSummary: '/api/ledger/summary',
        ledgerClearAttempts: '/api/ledger/clear-attempts',
        availability: '/api/availability',
        availabilityClear: '/api/availability/clear',
        hold: '/api/hold',
        holdSet: '/api/hold/set',
        holdClear: '/api/hold/clear',
        dispatch: '/api/dispatch',
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
      const profile = typeof req.query.profile === 'string' ? req.query.profile : undefined;
      const health = await registryService.checkNodeHealth(req.params.nodeId, profile);
      res.json(health);
    } catch (error) {
      res.status(404).json({
        error: 'Not Found',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/registry/fleet', async (req, res) => {
    try {
      const profile = typeof req.query.profile === 'string' ? req.query.profile : undefined;
      res.json(await registryService.getNodeObservations(profile));
    } catch (error) {
      res.status(500).json({
        error: 'Internal Server Error',
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

  // Central claim arbitration (issue #882). All three verify the calling
  // node is actually registered and has declared the profile it's trying
  // to claim -- a node can't claim work under a profile it never told the
  // registry it runs.
  function authorizeClaimRequest(nodeId: unknown, profile: unknown, workId: unknown): { nodeId: string; profile: string; workId: string } {
    if (typeof nodeId !== 'string' || !nodeId) throw new Error('Missing or invalid node_id');
    if (typeof profile !== 'string' || !profile) throw new Error('Missing or invalid profile');
    if (typeof workId !== 'string' || !workId) throw new Error('Missing or invalid work_id');
    const node = registryService.getNode(nodeId);
    if (!node) throw new Error(`Node '${nodeId}' is not registered`);
    if (!node.profiles?.includes(profile)) {
      throw new Error(`Node '${nodeId}' has not declared profile '${profile}' at registration`);
    }
    return { nodeId, profile, workId };
  }

  app.post('/api/claims/acquire', (req, res) => {
    try {
      const { nodeId, profile, workId } = authorizeClaimRequest(req.body.node_id, req.body.profile, req.body.work_id);
      const lease = claimsService.acquire(nodeId, profile, workId, req.body.lease_seconds);
      res.status(200).json(lease);
    } catch (error) {
      if (error instanceof ClaimConflictError) {
        res.status(409).json({ error: 'Conflict', message: error.message, held_by: error.heldBy });
        return;
      }
      res.status(400).json({ error: 'Bad Request', message: error instanceof Error ? error.message : String(error) });
    }
  });

  app.post('/api/claims/renew', (req, res) => {
    try {
      const { nodeId, profile, workId } = authorizeClaimRequest(req.body.node_id, req.body.profile, req.body.work_id);
      const lease = claimsService.renew(nodeId, profile, workId, req.body.lease_seconds);
      res.status(200).json(lease);
    } catch (error) {
      if (error instanceof ClaimConflictError) {
        res.status(409).json({ error: 'Conflict', message: error.message, held_by: error.heldBy });
        return;
      }
      res.status(400).json({ error: 'Bad Request', message: error instanceof Error ? error.message : String(error) });
    }
  });

  app.post('/api/claims/release', (req, res) => {
    try {
      const { nodeId, profile, workId } = authorizeClaimRequest(req.body.node_id, req.body.profile, req.body.work_id);
      claimsService.release(nodeId, profile, workId);
      res.status(200).json({ success: true });
    } catch (error) {
      res.status(400).json({ error: 'Bad Request', message: error instanceof Error ? error.message : String(error) });
    }
  });

  // Pull-data REST endpoints (TICKET-productization): these are on-demand
  // fetches -- report parameters, one work item's timeline -- that don't
  // fit the WS welcome message's connect-once push shape. Live/push data
  // (sessions, provider status) stays on the WebSocket; this is
  // additive, it does not replace or narrow the existing WS contract.
  //
  // `/api/status` fans out to registered nodes and returns an aggregated
  // fleet snapshot, so it must be auth-gated even though loopback callers may
  // still access it without credentials via authMiddleware's local exemption.
  app.get('/api/status', authMiddleware, async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      const [status, nodes] = await Promise.all([
        runStatus(profile),
        registryService.getNodeObservations(profile)
      ]);
      const identity = getCoordinatorIdentity(undefined, coordinatorPort);
      const enriched = {
        ...status,
        node_id: identity.node_id,
        display_name: identity.display_name,
        advertised_url: identity.advertised_url,
        version: identity.version,
        schema_digest: identity.schema_digest,
        resource_pressure: getLocalResourcePressure(),
        event_cursor: status.recent_ledger?.most_recent_dispatch_timestamp ?? status.generated_at,
        nodes
      };
      res.json(enriched);
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

  app.get('/api/sync', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      res.json(await runSync({ profile }));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah sync',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/ledger/summary', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : undefined;
    const since = typeof req.query.since === 'string' ? req.query.since : undefined;
    const groupByRaw = typeof req.query.groupBy === 'string' ? req.query.groupBy : undefined;
    const groupBy = groupByRaw === 'backend' || groupByRaw === 'model' ? groupByRaw : undefined;
    try {
      res.json(await runLedgerSummary({ profile, since, groupBy }));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah ledger summary',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/ledger/clear-attempts', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const workId = typeof req.body?.workId === 'string' ? req.body.workId : undefined;
    const dryRun = req.body?.dryRun === true;
    if (!workId) {
      res.status(400).json({ error: 'workId is required' });
      return;
    }
    try {
      await runLedgerClearAttempts({ profile, workId, dryRun });
      res.json({ ok: true });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to clear ledger attempts',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/availability', async (_req, res) => {
    try {
      res.json(await runAvailability());
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah availability',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/availability/clear', async (req, res) => {
    const backend = typeof req.body?.backend === 'string' ? req.body.backend : undefined;
    const backendInstance = typeof req.body?.backendInstance === 'string' ? req.body.backendInstance : undefined;
    const model = typeof req.body?.model === 'string' ? req.body.model : undefined;
    const quotaPool = typeof req.body?.quotaPool === 'string' ? req.body.quotaPool : undefined;
    if (!backend) {
      res.status(400).json({ error: 'backend is required' });
      return;
    }
    try {
      await runAvailabilityClear({ backend, backendInstance, model, quotaPool });
      res.json({ ok: true });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to clear gah availability',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Manager review-hold state has no standalone `gah hold list` -- it's
  // surfaced as StatusSnapshot.review_held_work_ids, so this reads status
  // rather than a dedicated CLI subcommand.
  app.get('/api/hold', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      const status = await runStatus(profile);
      res.json({ profile, workIds: status.review_held_work_ids ?? [] });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah hold state',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/hold/set', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const workId = typeof req.body?.workId === 'string' ? req.body.workId : undefined;
    const reason = typeof req.body?.reason === 'string' ? req.body.reason : undefined;
    if (!workId) {
      res.status(400).json({ error: 'workId is required' });
      return;
    }
    try {
      await runHoldSet({ profile, workId, reason });
      res.json({ ok: true });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to set gah hold',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/hold/clear', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const workId = typeof req.body?.workId === 'string' ? req.body.workId : undefined;
    if (!workId) {
      res.status(400).json({ error: 'workId is required' });
      return;
    }
    try {
      await runHoldClear({ profile, workId });
      res.json({ ok: true });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to clear gah hold',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Submits a dispatch as a fleet Session and returns immediately once it's
  // created -- progress streams over the existing WebSocket push bus (the
  // same mechanism the WS session.start path already uses), rather than
  // this route blocking for the full dispatch run or reimplementing a
  // second streaming transport.
  app.post('/api/dispatch', async (req, res) => {
    const body = req.body ?? {};
    const profile = typeof body.profile === 'string' ? body.profile : undefined;
    const providerKind = typeof body.providerKind === 'string' ? body.providerKind : undefined;
    const instanceId = typeof body.instanceId === 'string' ? body.instanceId : undefined;
    const repo = typeof body.repo === 'string' ? body.repo : undefined;
    const mode = typeof body.mode === 'string' ? body.mode : undefined;
    if (!profile || !providerKind || !instanceId || !repo || !mode) {
      res.status(400).json({ error: 'profile, providerKind, instanceId, repo, and mode are required' });
      return;
    }
    const options: SessionOptions & { requestId?: string; nodeId?: string; coordinatorNodeId?: string } = {
      profile,
      providerKind: providerKind as SessionOptions['providerKind'],
      instanceId: instanceId as SessionOptions['instanceId'],
      repo,
      mode,
      branch: typeof body.branch === 'string' ? body.branch : undefined,
      target: typeof body.target === 'string' ? body.target : undefined,
      backend: typeof body.backend === 'string' ? body.backend : undefined,
      model: typeof body.model === 'string' ? body.model : undefined,
      budget: typeof body.budget === 'number' ? body.budget : undefined,
      dryRun: body.dryRun === true,
      retries: typeof body.retries === 'number' ? body.retries : undefined,
      allowDraftFail: body.allowDraftFail === true,
      prod: body.prod === true,
      allowUnknownRedBaseline: body.allowUnknownRedBaseline === true,
      escalate: body.escalate === true,
      requestId: typeof body.requestId === 'string' ? body.requestId : undefined,
      nodeId: typeof body.nodeId === 'string' ? body.nodeId : undefined,
      coordinatorNodeId: typeof body.coordinatorNodeId === 'string' ? body.coordinatorNodeId : undefined
    };
    try {
      const session = await getFleetDispatch().startSession(options);
      res.status(202).json({ session });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to start dispatch session',
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
    // Issue #635 AC3: a missing required field must fail closed with 4xx
    // before ever reaching the CLI, not surface as an opaque 502 once
    // `gah profile add` itself rejects it.
    const REQUIRED_PROFILE_FIELDS: (keyof ProfileAddOptions)[] = [
      'name',
      'display_name',
      'repo_id',
      'provider',
      'repo',
      'local_path',
      'artifact_root',
    ];
    const missing = REQUIRED_PROFILE_FIELDS.filter(
      (field) => typeof req.body?.[field] !== 'string' || req.body[field].trim() === ''
    );
    if (missing.length > 0) {
      res.status(400).json({
        error: 'Invalid profile',
        message: `Missing required field(s): ${missing.join(', ')}`
      });
      return;
    }
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

  // Manager Chat backend selection: a default backend plus optional
  // per-profile overrides. Deliberately separate from `current_manager`
  // above -- that field drives the autonomous manager-wake notification
  // path (fire-and-forget, no session continuity, free-text with no
  // validation against a known backend list) and could reasonably diverge
  // from which backend answers the interactive chat page.
  app.get('/api/manager-chat/settings', (_req, res) => {
    res.json({ ...readManagerChatSettings(), availableBackends: listManagerBackends() });
  });

  app.post('/api/manager-chat/settings', (req, res) => {
    try {
      const current = readManagerChatSettings();
      const defaultBackend = typeof req.body?.defaultBackend === 'string' ? req.body.defaultBackend : current.defaultBackend;
      const profileOverrides =
        typeof req.body?.profileOverrides === 'object' && req.body.profileOverrides !== null
          ? req.body.profileOverrides
          : current.profileOverrides;
      writeManagerChatSettings({ defaultBackend, profileOverrides, modelOverrides: current.modelOverrides });
      res.json({ success: true });
    } catch (error) {
      res.status(400).json({
        error: 'Failed to update manager chat settings',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Real slash commands for the active backend, sourced live from the
  // backend's own command registry (e.g. Hermes's ACP available-commands
  // push) -- not something GAH invents. Powers the "/" palette.
  app.get('/api/manager-chat/commands', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      const commands = await listManagerChatCommands(profile);
      res.json({ commands });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load manager chat commands',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Real selectable models for the active backend, sourced live from its
  // own ACP session state -- not a list GAH maintains. Empty for backends
  // that don't expose this (Claude's ACP bridge doesn't today).
  app.get('/api/manager-chat/models', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      const summary = await listManagerChatModels(profile);
      res.json(summary);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load manager chat models',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/manager-chat/model', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const modelId = typeof req.body?.modelId === 'string' ? req.body.modelId : undefined;
    if (!modelId) {
      res.status(400).json({ error: 'Missing required field: modelId' });
      return;
    }
    try {
      await setManagerChatModel(profile, modelId);
      res.json({ success: true });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to set manager chat model',
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
