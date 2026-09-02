import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import type { CallToolResult } from '@modelcontextprotocol/sdk/types.js';
import { z } from 'zod';
import { gah, GahApiError } from './gahClient.js';

const DEFAULT_PROFILE = process.env.GAH_PROFILE ?? 'gah';

function ok(data: unknown): CallToolResult {
  return { content: [{ type: 'text', text: JSON.stringify(data, null, 2) }] };
}

function fail(error: unknown): CallToolResult {
  const message = error instanceof GahApiError ? `${error.message} (HTTP ${error.status})` : String(error);
  return { content: [{ type: 'text', text: message }], isError: true };
}

async function tool(handler: () => Promise<unknown>): Promise<CallToolResult> {
  try {
    return ok(await handler());
  } catch (error) {
    return fail(error);
  }
}

const profileArg = z.string().optional().describe('GAH profile name; defaults to GAH_PROFILE / "gah"');

export function createGahMcpServer(): McpServer {
  const server = new McpServer({ name: 'gah', version: '0.1.0' });

  server.registerTool(
    'gah_info',
    { title: 'GAH server info', description: 'Identify the connected GAH control-plane node and API version.' },
    () => tool(() => gah.info())
  );

  server.registerTool(
    'gah_status',
    {
      title: 'GAH status',
      description: 'Full status snapshot for a profile: merge requests, blockers, availability, ledger summary.',
      inputSchema: { profile: profileArg }
    },
    ({ profile }) => tool(() => gah.status(profile ?? DEFAULT_PROFILE))
  );

  server.registerTool(
    'gah_quota',
    {
      title: 'GAH quota snapshot',
      description: 'Usage/quota snapshot for a profile over a time window.',
      inputSchema: { profile: profileArg, since: z.string().optional().describe('e.g. "7d"') }
    },
    ({ profile, since }) => tool(() => gah.quota(profile ?? DEFAULT_PROFILE, since))
  );

  server.registerTool(
    'gah_usage_rollup',
    {
      title: 'GAH usage rollup',
      description: 'Actual manager-chat usage by day, backend, and model; use days=30 for a monthly view.',
      inputSchema: {
        profile: profileArg,
        days: z.number().int().min(1).max(90).default(30).describe('Number of days to include (1-90).')
      }
    },
    ({ profile, days }) => tool(() => gah.usageRollup(profile ?? DEFAULT_PROFILE, days))
  );

  server.registerTool(
    'gah_doctor',
    {
      title: 'GAH doctor',
      description: 'Run readiness checks for a profile (auth, config, backend availability).',
      inputSchema: { profile: profileArg }
    },
    ({ profile }) => tool(() => gah.doctor(profile ?? DEFAULT_PROFILE))
  );

  server.registerTool(
    'gah_report',
    {
      title: 'GAH report',
      description: 'Aggregate usage/cost/success-rate report, optionally grouped by backend or model.',
      inputSchema: {
        profile: profileArg,
        since: z.string().optional(),
        groupBy: z.enum(['backend', 'model']).optional()
      }
    },
    ({ profile, since, groupBy }) => tool(() => gah.report(profile, since, groupBy))
  );

  server.registerTool(
    'gah_profiles',
    { title: 'List GAH profiles', description: 'List all configured GAH profiles.' },
    () => tool(() => gah.profiles())
  );

  server.registerTool(
    'gah_work_history',
    {
      title: 'Work item ledger history',
      description: 'Full chronological ledger history (all attempts) for one work item.',
      inputSchema: { workId: z.string() }
    },
    ({ workId }) => tool(() => gah.workHistory(workId))
  );

  server.registerTool(
    'gah_sync',
    {
      title: 'GAH sync',
      description: 'Classified open (and recently resolved) merge requests/pull requests for a profile.',
      inputSchema: { profile: profileArg }
    },
    ({ profile }) => tool(() => gah.sync(profile ?? DEFAULT_PROFILE))
  );

  server.registerTool(
    'gah_ledger_summary',
    {
      title: 'GAH ledger summary',
      description: 'Aggregate ledger counts (success/fail, by mode/backend/model, token usage) over a window.',
      inputSchema: {
        profile: profileArg,
        since: z.string().optional().describe('e.g. "7d"'),
        groupBy: z.enum(['backend', 'model']).optional()
      }
    },
    ({ profile, since, groupBy }) => tool(() => gah.ledgerSummary(profile, since, groupBy))
  );

  server.registerTool(
    'gah_ledger_clear_attempts',
    {
      title: 'Clear ledger attempts',
      description: 'Append a tombstone ledger entry so a stuck work_id becomes dispatchable again.',
      inputSchema: { profile: profileArg, workId: z.string(), dryRun: z.boolean().optional() }
    },
    ({ profile, workId, dryRun }) => tool(() => gah.ledgerClearAttempts(profile ?? DEFAULT_PROFILE, workId, dryRun))
  );

  server.registerTool(
    'gah_availability',
    {
      title: 'GAH availability',
      description: 'Durable backend/model availability state, global (not per-profile).'
    },
    () => tool(() => gah.availability())
  );

  server.registerTool(
    'gah_availability_clear',
    {
      title: 'Clear availability override',
      description: "Override a stale unavailable record once the backend is confirmed healthy again.",
      inputSchema: {
        backend: z.string(),
        backendInstance: z.string().optional(),
        model: z.string().optional(),
        quotaPool: z.string().optional()
      }
    },
    ({ backend, backendInstance, model, quotaPool }) =>
      tool(() => gah.availabilityClear(backend, backendInstance, model, quotaPool))
  );

  server.registerTool(
    'gah_hold',
    {
      title: 'List review holds',
      description: 'Work IDs currently under an out-of-band manager review hold for a profile.',
      inputSchema: { profile: profileArg }
    },
    ({ profile }) => tool(() => gah.hold(profile ?? DEFAULT_PROFILE))
  );

  server.registerTool(
    'gah_hold_set',
    {
      title: 'Set a review hold',
      description: "Mark a work_id as under active out-of-band manager review; gah's auto-merge loop will skip it.",
      inputSchema: { profile: profileArg, workId: z.string(), reason: z.string().optional() }
    },
    ({ profile, workId, reason }) => tool(() => gah.holdSet(profile ?? DEFAULT_PROFILE, workId, reason))
  );

  server.registerTool(
    'gah_hold_clear',
    {
      title: 'Clear a review hold',
      description: 'Release a previously set review hold on a work_id.',
      inputSchema: { profile: profileArg, workId: z.string() }
    },
    ({ profile, workId }) => tool(() => gah.holdClear(profile ?? DEFAULT_PROFILE, workId))
  );

  server.registerTool(
    'gah_events',
    {
      title: 'GAH events',
      description: 'Recent controller and dispatch events for a profile.',
      inputSchema: { profile: profileArg, since: z.string().optional().describe('e.g. "24h" or "7d"') }
    },
    ({ profile, since }) => tool(() => gah.events(profile ?? DEFAULT_PROFILE, since))
  );

  server.registerTool(
    'gah_controller_activity',
    {
      title: 'GAH controller activity',
      description: 'Summarized agent/controller activity for a profile.',
      inputSchema: { profile: profileArg, since: z.string().optional().describe('e.g. "24h" or "7d"') }
    },
    ({ profile, since }) => tool(() => gah.controllerActivity(profile ?? DEFAULT_PROFILE, since))
  );

  server.registerTool(
    'gah_loop_status',
    {
      title: 'GAH loop status',
      description: 'Report whether the autonomous GAH loop is running for a profile.',
      inputSchema: { profile: profileArg }
    },
    ({ profile }) => tool(() => gah.loopStatus(profile ?? DEFAULT_PROFILE))
  );

  server.registerTool(
    'gah_dispatch',
    {
      title: 'Dispatch a GAH job',
      description:
        'Submit a dispatch as a fleet session and wait for its terminal push event by default. ' +
        'Set waitForCompletion=false to return immediately with the running session.',
      inputSchema: {
        profile: profileArg,
        providerKind: z.enum(['github', 'gitlab']),
        instanceId: z.string(),
        repo: z.string(),
        mode: z.string().describe('e.g. improve, fix, pm, review, experiment'),
        branch: z.string().optional(),
        target: z.string().optional(),
        backend: z.string().optional(),
        model: z.string().optional(),
        budget: z.number().optional(),
        dryRun: z.boolean().optional(),
        retries: z.number().int().min(0).optional(),
        allowDraftFail: z.boolean().optional(),
        waitForCompletion: z.boolean().default(true),
        waitTimeoutSeconds: z.number().int().min(1).max(7_200).default(3_600),
        requestId: z.string().optional(),
        nodeId: z.string().optional(),
        coordinatorNodeId: z.string().optional()
      }
    },
    (args) => tool(() => gah.dispatch({ ...args, profile: args.profile ?? DEFAULT_PROFILE }))
  );

  return server;
}
