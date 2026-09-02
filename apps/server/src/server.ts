import express from 'express';
import cors from 'cors';
import rateLimit from 'express-rate-limit';
import os from 'node:os';
import { statfsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
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
  SettingsConfigProfileSummary,
  DoctorSnapshot,
  ProfileSummary,
  ProjectImportData,
  ProjectImportResult,
  ChatNodeInfo,
  ChatSessionEvent
} from '@git-agent-harness/contracts';
import { getFleetDispatch } from './wsServer.js';
import type { SessionOptions } from './sessions/SessionManager.js';
import { deriveControllerActivity } from './controllerActivity.js';
import { authMiddleware, coordinatorTokenMatches } from './authMiddleware.js';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { RegistryService } from './registryService.js';
import { ClaimsService, ClaimConflictError } from './claimsService.js';
import { readSettings as readManagerChatSettings, writeSettings as writeManagerChatSettings } from './managerChat/settingsStore.js';
import { gatewayBaseUrl, gatewayApiKey, gatewayHealth, recall } from './managerChat/memoryGatewayClient.js';
import { readGatewaySettings, writeGatewaySettings } from './gatewaySettingsStore.js';
import { detectTailscaleIPv4 } from './tailscaleDetect.js';
import { listManagerBackends } from './managerChat/registry.js';
import {
  listCommandsForProfile as listManagerChatCommands,
  listModelsForProfile as listManagerChatModels,
  listModelsForBackend as listManagerChatModelsForBackend,
  setModelForProfile as setManagerChatModel,
  setReasoningEffortForProfile as setManagerChatReasoningEffort,
  listChatSessions,
  createChatSession,
  archiveChatSession,
  updateChatSession,
  getChatPreview as getManagerChatPreview,
  setChatPreview as setManagerChatPreview,
  listChatIssuesForProfile as listManagerChatIssues,
  startChatFromIssue as startManagerChatFromIssue,
  listChatPrsForProfile as listManagerChatPrs,
  startChatFromPr as startManagerChatFromPr,
  enqueueManagerWake as enqueueManagerChatWake
} from './managerChat/ManagerChatManager.js';
import { reclaimChatSessions } from './managerChat/chatMaintenance.js';
import { listAllChatSessions } from './managerChat/chatSessions.js';
import { usageRollup } from './managerChat/usageRollup.js';
import { addProject, importGitProject, listProjects, parseGitUrl, removeProject } from './projectCatalog.js';
import { getGitStatusCached, getGitBranchesCached, getGitLogCached, getGitPrsCached, commitGitChanges } from './gitCache.js';
import {
  addCanonicalSkillBinding,
  clearProfileSkillBindings,
  deleteSkill,
  getSkill,
  listSkillSummaries,
  listSkills,
  putSkill,
  resolveSkillBindings,
  seedSkillFromDocs,
  setProfileSkillBindings,
  skillBindingSummary
} from './skillBank.js';
import { readLog as readManagerChatLog } from './managerChat/sessionLog.js';
import {
  getPendingCommits,
  readAdminUpdateState,
  startAdminUpdate,
  type AdminUpdatePendingInfo,
  type AdminUpdateState,
  type StartAdminUpdateResult
} from './adminUpdate.js';

const SERVER_VERSION = '0.1.0';

type ConfigEffectiveDeps = {
  runConfigShowProfile: (profile: string) => Promise<ConfigProfileSummary>;
  runDoctor: (profile: string) => Promise<DoctorSnapshot>;
};

type CreateServerOptions = Partial<ConfigEffectiveDeps> & {
  runProfileList?: () => Promise<ProfileSummary[]>;
  runProfileAdd?: (options: ProfileAddOptions) => Promise<void>;
  registryService?: RegistryService;
  claimsService?: ClaimsService;
  coordinatorPort?: number;
  getPendingCommits?: typeof getPendingCommits;
  startAdminUpdate?: typeof startAdminUpdate;
  readAdminUpdateState?: typeof readAdminUpdateState;
  detectTailscaleIPv4?: typeof detectTailscaleIPv4;
};

const DEFAULT_CONFIG_EFFECTIVE_DEPS: ConfigEffectiveDeps = {
  runConfigShowProfile,
  runDoctor
};

/** Validate and seed the central skill bank before the HTTP server starts. */
export function initializeSkillBank(): void {
  listSkills();
  const docsPath = process.env.GAH_SKILL_DOCS_PATH
    || fileURLToPath(new URL('../../../docs/gah-manager-skill.md', import.meta.url));
  try {
    seedSkillFromDocs(
      docsPath,
      'docs/gah-manager-skill.md',
      'gah-manager',
      '1.0.0',
      'GAH Project Manager & Orchestrator',
      'The GAH manager agent skill: sits above worker agents to break down issues, enforce policies, and dispatch isolated tasks.',
      ['hermes', 'codex', 'claude', 'opencode']
    );
  } catch (error) {
    console.error('Failed to seed skill bank from docs:', error);
  }
  if (getSkill('gah-manager')) {
    for (const backend of ['hermes', 'codex', 'claude', 'opencode']) {
      addCanonicalSkillBinding('gah-manager', backend);
    }
  }
}

/** Same hardcoded default as wsServer.ts's welcome message, until Settings
 * gains real profile switching (see apps/web Settings page). */
const DEFAULT_PROFILE = 'gah';

function observedSkills(profile: string, backend: string, sessionId?: string): { id: string; version: string }[] | null {
  const options = sessionId && sessionId !== 'default' ? { sessionId } : {};
  const applied = readManagerChatLog(profile, options)
    .reverse()
    .find((event: ChatSessionEvent) => event.type === 'skills/applied' && event.backend === backend);
  return applied?.type === 'skills/applied' ? applied.skills : null;
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

function gatewayPort(url: string): string {
  try {
    return new URL(url).port || '8420';
  } catch {
    return '8420';
  }
}

function toSettingsConfigProfileSummary(config: ConfigProfileSummary): SettingsConfigProfileSummary {
  const { env_file, env_file_prod, ...notifications } = config.notifications;
  return {
    ...config,
    notifications: {
      ...notifications,
      env_file_configured: env_file != null,
      env_file_prod_configured: env_file_prod != null
    }
  };
}

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
  const listProfiles = configDeps.runProfileList ?? runProfileList;
  const addProfile = configDeps.runProfileAdd ?? runProfileAdd;
  const getPendingCommitsFn = configDeps.getPendingCommits ?? getPendingCommits;
  const startAdminUpdateFn = configDeps.startAdminUpdate ?? startAdminUpdate;
  const readAdminUpdateStateFn = configDeps.readAdminUpdateState ?? readAdminUpdateState;
  const detectTailscaleIPv4Fn = configDeps.detectTailscaleIPv4 ?? detectTailscaleIPv4;
  const gatewaySettingsSummary = async () => {
    const apiKey = gatewayApiKey();
    const stored = readGatewaySettings();
    return {
      url: gatewayBaseUrl(),
      apiKeyConfigured: !!apiKey,
      enabled: stored.enabled,
      disabledProfiles: stored.disabledProfiles,
      contextPolicy: stored.contextPolicy,
      contextPolicies: stored.contextPolicies,
      degraded: gatewayHealth(),
      tailscaleIPv4: await detectTailscaleIPv4Fn()
    };
  };

  const registryService =
    configDeps.registryService ||
    new RegistryService(undefined, getCoordinatorIdentity(undefined, coordinatorPort).advertised_url, coordinatorPort);
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
  // authMiddleware guards new, narrowly scoped sensitive surfaces. The rest
  // of the API (loop start/stop, legacy config mutation, etc.) is
  // unauthenticated pending #532; applying this globally would silently change
  // that pre-existing contract.
  app.use('/api/registry', authMiddleware);
  app.use('/api/claims', authMiddleware);
  // /api/settings/gateway includes an explicit endpoint that reveals a
  // credential-bearing bootstrap command, so the whole narrow surface gets
  // the same gate as registry/claims rather than the unauthenticated default.
  app.use('/api/settings', authMiddleware);
  app.use('/api/projects', authMiddleware);
  // /api/skills (issue #963/#964): the central skill bank mutates the
  // versioned store, so it gets the same narrow auth gate as projects.
  app.use('/api/skills', authMiddleware);
  // /api/admin/update (issue #989) shells out to `gah update`, which runs
  // arbitrary build commands and restarts the control-plane service -- the
  // same auth gate as registry/claims/settings, plus an explicit opt-in env
  // flag so this surface doesn't exist at all unless an operator enables it.
  app.use('/api/admin', authMiddleware);
  app.use('/api/manager-chat/wake', authMiddleware);
  app.use('/api/admin', (req, res, next) => {
    if (process.env.GAH_ENABLE_ADMIN_UPDATE !== '1') {
      return res.status(404).json({
        error: 'Not Found',
        message: 'Admin update is disabled; set GAH_ENABLE_ADMIN_UPDATE=1 on the server to enable it.'
      });
    }
    next();
  });
  app.use('/api/projects/import', rateLimit({
    windowMs: 60_000,
    limit: 10,
    standardHeaders: true,
    legacyHeaders: false
  }));
  // Issue #882 (CodeQL: js/missing-rate-limiting) -- these routes are
  // authenticated but called frequently by design (a renewal every
  // lease/3, ~5 min, per in-flight dispatch), so the limit is generous for
  // legitimate traffic and exists as defense-in-depth against a buggy or
  // compromised node hammering the endpoint, not to throttle normal use.
  app.use(
    '/api/claims',
    rateLimit({
      windowMs: 60_000,
      limit: 60,
      standardHeaders: true,
      legacyHeaders: false
    })
  );

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
        projects: '/api/projects',
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
      const nodeId = (req.body as { node_id?: unknown } | undefined)?.node_id;
      // Ownership check (#951 review): creating a NEW node is how a worker
      // self-registers, so it stays as open as authMiddleware already allows.
      // UPDATING an existing node_id repoints where the central polls
      // (advertised_url) and how it authenticates (secret_ref), so it must
      // prove it knows the coordinator token -- authMiddleware alone can't
      // gate this because a reverse-proxied LAN peer appears loopback to
      // Express and skips auth entirely.
      if (typeof nodeId === 'string' && registryService.getNode(nodeId)) {
        const authHeader = req.headers.authorization ?? '';
        const token = authHeader.startsWith('Bearer ') ? authHeader.slice(7) : '';
        if (!coordinatorTokenMatches(token)) {
          return res.status(403).json({
            error: 'Forbidden',
            message: 'Updating an existing node registration requires the coordinator token'
          });
        }
      }
      const { warnings, created } = registryService.registerNode(req.body);
      res.status(created ? 201 : 200).json({
        success: true,
        message: created ? 'Node registered successfully' : 'Node registration updated',
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

  // Actual usage observed by GAH itself, aggregated from the manager-chat
  // session logs (#940): the dispatch ledger and account-level quota checks
  // don't cover manager-chat turns, so subscription-burn visibility comes
  // from here.
  app.get('/api/usage/rollup', (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const rawDays = Number.parseInt(typeof req.query.days === 'string' ? req.query.days : '', 10);
    const days = Number.isFinite(rawDays) ? Math.min(Math.max(rawDays, 1), 90) : 7;
    try {
      res.json(usageRollup(profile, days));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to roll up manager-chat usage',
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

  // Submits a dispatch as a fleet Session. Callers may opt into a single
  // push-driven wait for its terminal event; the default remains immediate.
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
      mr: typeof body.mr === 'string' ? body.mr : undefined,
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
      const fleetDispatch = getFleetDispatch();
      const session = await fleetDispatch.startSession(options);
      if (body.waitForCompletion === true) {
        const requestedSeconds = typeof body.waitTimeoutSeconds === 'number'
          ? body.waitTimeoutSeconds
          : 3_600;
        const timeoutMs = Math.max(1, Math.min(7_200, requestedSeconds)) * 1_000;
        const terminal = await fleetDispatch.waitForSession(session.id, timeoutMs);
        res.status(terminal.timedOut ? 202 : 200).json(terminal);
        return;
      }
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
      const profiles = await listProfiles();
      res.json(profiles);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load gah profiles',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/projects', async (_req, res) => {
    try {
      res.json(listProjects(await listProfiles()));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load projects',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/projects', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile.trim() : '';
    if (!profile) {
      res.status(400).json({ error: 'Invalid project', message: 'profile is required' });
      return;
    }
    try {
      res.status(201).json(addProject(profile, await listProfiles()));
    } catch (error) {
      res.status(400).json({
        error: 'Failed to add project',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/projects/import', async (req, res) => {
    const gitUrl = typeof req.body?.gitUrl === 'string' ? req.body.gitUrl.trim() : '';
    if (!gitUrl) {
      res.status(400).json({ error: 'Invalid project import', message: 'gitUrl is required' });
      return;
    }
    try {
      parseGitUrl(gitUrl);
    } catch (error) {
      res.status(400).json({
        error: 'Invalid project import',
        message: error instanceof Error ? error.message : String(error)
      });
      return;
    }
    try {
      const input: ProjectImportData = { gitUrl, reclone: req.body?.reclone === true };
      const imported = await importGitProject(input, { listProfiles, addProfile });
      const project = addProject(imported.profileName, await listProfiles());
      const result: ProjectImportResult = {
        project,
        checkoutPath: imported.checkoutPath,
        checkoutStatus: imported.checkoutStatus,
        detectedLanguages: imported.detectedLanguages,
        validationCommands: imported.validationCommands
      };
      res.status(201).json(result);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const conflict = message.includes('uncommitted changes')
        || message.includes('checkout origin')
        || message.includes('managed checkouts');
      res.status(conflict ? 409 : 502).json({
        error: 'Failed to import project',
        message
      });
    }
  });

  app.delete('/api/projects/:profile', (req, res) => {
    try {
      res.json({ removed: removeProject(req.params.profile) });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to remove project',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // ── Skill bank (issue #963/#964) ───────────────────────────────────────

  app.get('/api/skills', (_req, res) => {
    try {
      res.json({ skills: listSkillSummaries() });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load skills',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/skills/bindings', (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : '';
    const backend = typeof req.query.backend === 'string' ? req.query.backend : '';
    const instance = typeof req.query.instance === 'string' ? req.query.instance : undefined;
    const sessionId = typeof req.query.sessionId === 'string' ? req.query.sessionId : undefined;
    if (!profile || !backend) {
      res.status(400).json({ error: 'Invalid skill binding', message: 'profile and backend are required' });
      return;
    }
    try {
      res.json(skillBindingSummary(profile, backend, instance, observedSkills(profile, backend, sessionId)));
    } catch (error) {
      res.status(400).json({
        error: 'Failed to resolve skill bindings',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/skills/resolve', (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : '';
    const backend = typeof req.query.backend === 'string' ? req.query.backend : '';
    const instance = typeof req.query.instance === 'string' ? req.query.instance : undefined;
    if (!profile || !backend) {
      res.status(400).json({ error: 'Invalid skill resolution', message: 'profile and backend are required' });
      return;
    }
    try {
      res.json(resolveSkillBindings(profile, backend, instance));
    } catch (error) {
      res.status(400).json({
        error: 'Failed to resolve skills',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.put('/api/skills/bindings', (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : '';
    const backend = typeof req.body?.backend === 'string' ? req.body.backend : '';
    const instance = typeof req.body?.instance === 'string' ? req.body.instance : undefined;
    const skillIds = Array.isArray(req.body?.skillIds)
      ? req.body.skillIds.filter((id: unknown): id is string => typeof id === 'string')
      : null;
    if (!profile || !backend || skillIds === null || skillIds.length !== req.body.skillIds.length) {
      res.status(400).json({ error: 'Invalid skill binding', message: 'profile, backend, and string skillIds are required' });
      return;
    }
    try {
      setProfileSkillBindings(profile, backend, skillIds, instance);
      res.json(skillBindingSummary(profile, backend, instance));
    } catch (error) {
      res.status(400).json({
        error: 'Failed to update skill bindings',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.delete('/api/skills/bindings', (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : '';
    const backend = typeof req.query.backend === 'string' ? req.query.backend : '';
    const instance = typeof req.query.instance === 'string' ? req.query.instance : undefined;
    if (!profile || !backend) {
      res.status(400).json({ error: 'Invalid skill binding', message: 'profile and backend are required' });
      return;
    }
    clearProfileSkillBindings(profile, backend, instance);
    res.json(skillBindingSummary(profile, backend, instance));
  });

  app.get('/api/skills/:id', (req, res) => {
    try {
      const skill = getSkill(req.params.id, typeof req.query.version === 'string' ? req.query.version : undefined);
      if (!skill) {
        res.status(404).json({ error: `Skill '${req.params.id}' not found` });
        return;
      }
      res.json(skill);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load skill',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/skills', (req, res) => {
    const body = req.body as {
      id?: unknown;
      version?: unknown;
      displayName?: unknown;
      description?: unknown;
      content?: unknown;
      backends?: unknown;
      source?: unknown;
    };
    if (typeof body.id !== 'string' || typeof body.version !== 'string' || typeof body.content !== 'string') {
      res.status(400).json({ error: 'Invalid skill', message: 'id, version, and content are required' });
      return;
    }
    try {
      const now = Date.now();
      const existing = getSkill(body.id, body.version);
      const skill = putSkill({
        id: body.id,
        version: body.version,
        displayName: typeof body.displayName === 'string' ? body.displayName : body.id,
        description: typeof body.description === 'string' ? body.description : '',
        content: body.content,
        backends: Array.isArray(body.backends) ? body.backends.filter((b): b is string => typeof b === 'string') : [],
        source: typeof body.source === 'string' ? body.source : 'api',
        createdAt: existing?.createdAt ?? now,
        updatedAt: now
      });
      res.status(existing ? 200 : 201).json(skill);
    } catch (error) {
      res.status(400).json({
        error: 'Failed to store skill',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.delete('/api/skills/:id', (req, res) => {
    try {
      const removed = deleteSkill(req.params.id);
      res.json({ removed: removed.length });
    } catch (error) {
      // Deletion refused because the skill is bound (AC7).
      res.status(409).json({
        error: 'Failed to delete skill',
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
      res.json(toSettingsConfigProfileSummary(config));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to load effective config',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Manager Chat backend selection: a default backend plus optional
  // per-profile overrides. Deliberately separate from `current_manager`
  // above -- that field selects the backend for autonomous wakes, while this
  // setting selects the backend for ordinary interactive chat turns.
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
      writeManagerChatSettings({
        defaultBackend,
        profileOverrides,
        modelOverrides: current.modelOverrides,
        reasoningEffortOverrides: current.reasoningEffortOverrides
      });
      res.json({ success: true });
    } catch (error) {
      res.status(400).json({
        error: 'Failed to update manager chat settings',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/manager-chat/wake', async (req, res) => {
    const manager = typeof req.body?.manager === 'string' ? req.body.manager : '';
    const repoId = typeof req.body?.repoId === 'string' ? req.body.repoId : '';
    const instruction = typeof req.body?.instruction === 'string' ? req.body.instruction : '';
    if (!manager || !repoId || !instruction) {
      res.status(400).json({ error: 'Missing required fields: manager, repoId, instruction' });
      return;
    }
    try {
      const matches = (await listProfiles()).filter((profile) => profile.repo_id === repoId);
      if (matches.length !== 1) {
        res.status(matches.length === 0 ? 404 : 409).json({
          error: matches.length === 0 ? 'Profile not found' : 'Ambiguous repoId',
          message: `Expected one profile for repoId '${repoId}', found ${matches.length}`
        });
        return;
      }
      enqueueManagerChatWake(matches[0].name, manager, instruction);
      res.status(202).json({ accepted: true, profile: matches[0].name });
    } catch (error) {
      res.status(400).json({
        error: 'Failed to enqueue manager wake',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Ordinary Settings reads are credential-free. An operator setting up a
  // second node must explicitly request the sensitive bootstrap command via
  // the authenticated POST below.
  app.get('/api/settings/gateway', async (_req, res) => {
    res.json(await gatewaySettingsSummary());
  });

  app.post('/api/settings/gateway/bootstrap-command', async (_req, res) => {
    res.set('Cache-Control', 'no-store');
    const apiKey = gatewayApiKey();
    if (!apiKey) {
      res.status(409).json({ message: 'Configure a gateway API key before revealing the setup command.' });
      return;
    }
    const tailscaleIPv4 = await detectTailscaleIPv4Fn();
    if (!tailscaleIPv4) {
      res.status(409).json({ message: "Couldn't detect this host's Tailscale address." });
      return;
    }
    const remoteGatewayUrl = `http://${tailscaleIPv4}:${gatewayPort(gatewayBaseUrl())}`;
    const command =
      'curl -fsSL https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh' +
      ` | GAH_GATEWAY_MODE=remote GAH_GATEWAY_URL=${shellQuote(remoteGatewayUrl)}` +
      ` GAH_GATEWAY_API_KEY=${shellQuote(apiKey)} bash`;
    res.json({ command });
  });

  app.put('/api/settings/gateway', async (req, res) => {
    const { url, apiKey, enabled, disabledProfiles, contextPolicy, contextPolicies } = req.body as {
      url?: string | null;
      apiKey?: string | null;
      enabled?: boolean;
      disabledProfiles?: string[];
      contextPolicy?: import('./gatewaySettingsStore.js').MemoryContextPolicy;
      contextPolicies?: Record<string, import('./gatewaySettingsStore.js').MemoryContextPolicy>;
    };
    const patch: Parameters<typeof writeGatewaySettings>[0] = {};
    if ('url' in req.body) patch.url = url ?? null;
    if ('apiKey' in req.body) patch.apiKey = apiKey ?? null;
    if (typeof enabled === 'boolean') patch.enabled = enabled;
    if (Array.isArray(disabledProfiles)) patch.disabledProfiles = disabledProfiles;
    if (contextPolicy && typeof contextPolicy === 'object') patch.contextPolicy = contextPolicy;
    if (contextPolicies && typeof contextPolicies === 'object') patch.contextPolicies = contextPolicies;
    writeGatewaySettings(patch);
    res.json(await gatewaySettingsSummary());
  });

  app.post('/api/context/recall', async (req, res) => {
    const { profile, query } = req.body as { profile?: string; query?: string };
    if (!query) return res.status(400).json({ error: 'query required' });
    const p = typeof profile === 'string' && profile ? profile : DEFAULT_PROFILE;
    // #878: recall is fail-open (never throws), so surface a degraded result
    // here explicitly -- this is a diagnostic endpoint, an empty 200 would
    // silently lie about why nothing came back.
    const result = await recall(p, query);
    if (result.degraded) {
      res.status(502).json({ error: result.error ?? 'memory gateway unreachable', degraded: true });
      return;
    }
    res.json(result);
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

  // Real selectable models and reasoning efforts for the active backend,
  // sourced live from its own ACP session state -- not lists GAH maintains.
  // `backend` overrides the profile default (new-chat flow shows
  // per-backend lists).
  app.get('/api/manager-chat/models', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const backend = typeof req.query.backend === 'string' ? req.query.backend : undefined;
    try {
      const summary = backend
        ? await listManagerChatModelsForBackend(profile, backend)
        : await listManagerChatModels(profile);
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

  // Capability-aware reasoning effort: values come from the active ACP
  // backend's thought_level config option and are validated by that adapter.
  app.post('/api/manager-chat/reasoning-effort', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const effortId = typeof req.body?.effortId === 'string' ? req.body.effortId : undefined;
    if (!effortId) {
      res.status(400).json({ error: 'Missing required field: effortId' });
      return;
    }
    try {
      await setManagerChatReasoningEffort(profile, effortId);
      res.json({ success: true });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to set manager chat reasoning effort',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // The new-chat flow's node step. Chat runs on the central node today;
  // registered workers are listed for fleet visibility but marked not yet
  // chat-capable (worker-side chat dispatch is future work).
  app.get('/api/manager-chat/nodes', (_req, res) => {
    const identity = getCoordinatorIdentity(undefined, coordinatorPort);
    const central: ChatNodeInfo = {
      nodeId: identity.node_id,
      displayName: identity.display_name,
      role: 'central',
      chatCapable: true,
      lastSeenAt: null
    };
    const workers: ChatNodeInfo[] = registryService
      .getNodesSummary()
      .filter((node) => node.node_id !== identity.node_id)
      .map((node) => ({
        nodeId: node.node_id,
        displayName: node.display_name,
        role: 'worker' as const,
        chatCapable: false,
        lastSeenAt: node.last_seen_at ?? null
      }));
    res.json({ nodes: [central, ...workers] });
  });

  // WP2 chat sessions: a session is one conversation bound to one worktree.
  // List/create/archive are plain REST (the UI's session rail); turns
  // themselves keep flowing over the WS manager.chat.* messages, which now
  // carry an optional sessionId.
  app.get('/api/manager-chat/sessions', (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    res.json({ sessions: listChatSessions(profile) });
  });

  // Cross-project listing for the chat picker: every project's sessions,
  // grouped and named from the profile list when it resolves (the picker
  // falls back to the raw profile id for projects the list doesn't cover).
  app.get('/api/manager-chat/sessions/all', async (_req, res) => {
    try {
      const profiles = await listProfiles().catch(() => []);
      const displayNames = new Map(profiles.map((p) => [p.name, p.display_name || p.name]));
      res.json({
        projects: listAllChatSessions().map((group) => ({
          profile: group.profile,
          profileName: displayNames.get(group.profile) ?? group.profile,
          sessions: group.sessions
        }))
      });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to list chat sessions',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.get('/api/manager-chat/storage', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      res.json(await reclaimChatSessions({ profile, dryRun: true }));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to inspect chat storage',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/manager-chat/reclaim', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : undefined;
    const dryRun = req.body?.dryRun !== false;
    try {
      res.json(await reclaimChatSessions({ profile, dryRun }));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to reclaim chat sessions',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/manager-chat/sessions', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const backend = typeof req.body?.backend === 'string' ? req.body.backend : undefined;
    const model = typeof req.body?.model === 'string' ? req.body.model : null;
    const title = typeof req.body?.title === 'string' ? req.body.title : undefined;
    try {
      const session = await createChatSession(profile, backend, model, title);
      res.status(201).json(session);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to create chat session',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/manager-chat/sessions/update', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const sessionId = typeof req.body?.sessionId === 'string' ? req.body.sessionId : undefined;
    if (!sessionId) {
      res.status(400).json({ error: 'Missing required field: sessionId' });
      return;
    }
    const patch: { backend?: string; model?: string | null; reasoningEffort?: string | null; title?: string } = {};
    if (typeof req.body?.backend === 'string') patch.backend = req.body.backend;
    if (typeof req.body?.model === 'string' || req.body?.model === null) patch.model = req.body.model;
    if (typeof req.body?.reasoningEffort === 'string' || req.body?.reasoningEffort === null) patch.reasoningEffort = req.body.reasoningEffort;
    if (typeof req.body?.title === 'string') patch.title = req.body.title;
    try {
      const session = await updateChatSession(profile, sessionId, patch);
      res.json(session);
    } catch (error) {
      res.status(502).json({
        error: 'Failed to update chat session',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/manager-chat/sessions/archive', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const sessionId = typeof req.body?.sessionId === 'string' ? req.body.sessionId : undefined;
    const sessionIds: string[] = Array.isArray(req.body?.sessionIds)
      ? req.body.sessionIds.filter((id: unknown): id is string => typeof id === 'string')
      : [];
    const requested = [...new Set(sessionId ? [sessionId] : sessionIds)].slice(0, 200);
    if (requested.length === 0) {
      res.status(400).json({ error: 'Missing required field: sessionId or sessionIds' });
      return;
    }
    try {
      const sessions = [];
      for (const id of requested) sessions.push(await archiveChatSession(profile, id));
      res.json(sessionId ? sessions[0] : { sessions });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to archive chat session',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // Issue → chat (issue-to-workflow): open issues for the project's repo,
  // and grab one into a chat -- branch for it, mark in progress, seed the
  // session with the issue body.
  app.get('/api/manager-chat/issues', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      res.json({ issues: await listManagerChatIssues(profile) });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to list issues',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/manager-chat/issues/start', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const issueNumber = typeof req.body?.issueNumber === 'number' ? req.body.issueNumber : undefined;
    const backend = typeof req.body?.backend === 'string' ? req.body.backend : undefined;
    const model = typeof req.body?.model === 'string' ? req.body.model : null;
    if (issueNumber === undefined) {
      res.status(400).json({ error: 'Missing required field: issueNumber' });
      return;
    }
    try {
      res.status(201).json(await startManagerChatFromIssue(profile, issueNumber, backend, model));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to start chat from issue',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // PR → chat: open PRs for the project's repo, and open a read-only chat
  // seeded with one -- no worktree, no branch, nothing at the provider is
  // touched (browsing a PR must never mutate it).
  app.get('/api/manager-chat/prs', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    try {
      res.json({ prs: await listManagerChatPrs(profile) });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to list pull requests',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  app.post('/api/manager-chat/prs/start', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const prNumber = typeof req.body?.prNumber === 'number' ? req.body.prNumber : undefined;
    const backend = typeof req.body?.backend === 'string' ? req.body.backend : undefined;
    const model = typeof req.body?.model === 'string' ? req.body.model : null;
    if (prNumber === undefined) {
      res.status(400).json({ error: 'Missing required field: prNumber' });
      return;
    }
    try {
      res.status(201).json(await startManagerChatFromPr(profile, prNumber, backend, model));
    } catch (error) {
      res.status(502).json({
        error: 'Failed to start chat from pull request',
        message: error instanceof Error ? error.message : String(error)
      });
    }
  });

  // WP3 session preview: the dedicated port proxying the session worktree's
  // dev server. GET the current state; POST sets (or clears with port:null)
  // — auto-detection from agent tool output pushes manager.chat.preview.
  app.get('/api/manager-chat/preview', (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const sessionId = typeof req.query.sessionId === 'string' ? req.query.sessionId : '';
    if (!sessionId) {
      res.status(400).json({ error: 'Missing required query param: sessionId' });
      return;
    }
    res.json({ preview: getManagerChatPreview(profile, sessionId) });
  });

  app.post('/api/manager-chat/preview/set', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile : DEFAULT_PROFILE;
    const sessionId = typeof req.body?.sessionId === 'string' ? req.body.sessionId : undefined;
    const port = req.body?.port === null ? null : typeof req.body?.port === 'number' ? req.body.port : undefined;
    if (!sessionId) {
      res.status(400).json({ error: 'Missing required field: sessionId' });
      return;
    }
    if (port === undefined) {
      res.status(400).json({ error: 'Field "port" must be a number or null' });
      return;
    }
    try {
      const preview = await setManagerChatPreview(profile, sessionId, port);
      res.json({ preview });
    } catch (error) {
      res.status(502).json({
        error: 'Failed to set preview',
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

  // Git integration: thin wrappers over git/gh/glab so the UI can show
  // branch/PR state without the user leaving the dashboard.
  async function resolveLocalPath(profile: string): Promise<string | null> {
    const profiles = await runProfileList();
    return profiles.find((p) => p.name === profile)?.local_path ?? null;
  }

  function cliInDir(bin: string, args: string[], cwd: string): { ok: boolean; out: string } {
    const r = spawnSync(bin, args, { cwd, encoding: 'utf8' });
    return { ok: r.status === 0, out: r.stdout ?? '' };
  }

  app.get('/api/git/status', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const cwd = await resolveLocalPath(profile);
    if (!cwd) return res.status(404).json({ error: 'Profile not found' });
    try {
      const result = await getGitStatusCached(profile, cwd);
      res.json(result);
    } catch (error) {
      res.status(502).json({ error: error instanceof Error ? error.message : String(error) });
    }
  });

  app.get('/api/git/branches', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const cwd = await resolveLocalPath(profile);
    if (!cwd) return res.status(404).json({ error: 'Profile not found' });
    try {
      const result = await getGitBranchesCached(profile, cwd);
      res.json(result);
    } catch (error) {
      res.status(502).json({ error: error instanceof Error ? error.message : String(error) });
    }
  });

  app.get('/api/git/log', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const limit = typeof req.query.limit === 'string' ? Math.min(50, parseInt(req.query.limit, 10) || 20) : 20;
    const cwd = await resolveLocalPath(profile);
    if (!cwd) return res.status(404).json({ error: 'Profile not found' });
    try {
      const result = await getGitLogCached(profile, cwd, limit);
      res.json(result);
    } catch (error) {
      res.status(502).json({ error: error instanceof Error ? error.message : String(error) });
    }
  });

  app.get('/api/git/prs', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const cwd = await resolveLocalPath(profile);
    if (!cwd) return res.status(404).json({ error: 'Profile not found' });
    const profiles = await runProfileList();
    const prof = profiles.find((p) => p.name === profile);
    const isGitLab = prof?.provider === 'gitlab';
    try {
      const result = await getGitPrsCached(profile, cwd, isGitLab);
      res.json(result);
    } catch (error) {
      res.json({ prs: [], warning: error instanceof Error ? error.message : 'Unknown error' });
    }
  });

  app.post('/api/git/pr', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const { title, body = '', base, draft = false } = req.body as { title?: string; body?: string; base?: string; draft?: boolean };
    if (!title) return res.status(400).json({ error: 'title required' });
    const cwd = await resolveLocalPath(profile);
    if (!cwd) return res.status(404).json({ error: 'Profile not found' });
    const args = ['pr', 'create', '--title', title, '--body', body];
    if (base) args.push('--base', base);
    if (draft) args.push('--draft');
    const { ok, out } = cliInDir('gh', args, cwd);
    if (!ok) return res.status(502).json({ error: 'gh pr create failed' });
    res.json({ url: out.trim() });
  });

  app.post('/api/git/commit', async (req, res) => {
    const profile = typeof req.query.profile === 'string' ? req.query.profile : DEFAULT_PROFILE;
    const { message } = req.body as { message?: string };
    if (!message || !message.trim()) return res.status(400).json({ error: 'message required' });
    const cwd = await resolveLocalPath(profile);
    if (!cwd) return res.status(404).json({ error: 'Profile not found' });
    try {
      const result = await commitGitChanges(profile, cwd, message.trim());
      res.json(result);
    } catch (error) {
      res.status(502).json({ error: error instanceof Error ? error.message : String(error) });
    }
  });

  // Issue #989: in-app update path. Operates on this server's own GAH
  // checkout (process.cwd(), the repo `gah-server.service`'s
  // WorkingDirectory points at) -- not a managed profile's repo.
  app.get('/api/admin/update', (req, res) => {
    const pending: AdminUpdatePendingInfo = getPendingCommitsFn(process.cwd());
    res.json(pending);
  });

  app.post('/api/admin/update', (req, res) => {
    const result: StartAdminUpdateResult = startAdminUpdateFn();
    res.status(result.started ? 202 : 409).json(result.state);
  });

  app.get('/api/admin/update/status', (req, res) => {
    const state: AdminUpdateState = readAdminUpdateStateFn();
    res.json(state);
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
