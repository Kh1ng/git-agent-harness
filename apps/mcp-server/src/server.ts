import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import type { CallToolResult } from '@modelcontextprotocol/sdk/types.js';
import { z } from 'zod';
import { gah, GahApiError } from './gahClient.js';


const DEFAULT_PROFILE = process.env.GAH_PROFILE ?? 'gah';

// Issue #525: tool input schemas are DERIVED from the capability manifest's
// embedded request JSON schemas — the Rust operation definitions are the
// single source of truth. The manifest ships in the contracts package dist;
// resolve it relative to this module (node_modules layout in dev and in the
// installed tree both place it at the same relative depth).
import { createRequire } from 'node:module';
const require_ = createRequire(import.meta.url);
function loadRequestSchemas(): Record<string, Record<string, unknown>> {
  try {
    const manifest = require_('@git-agent-harness/contracts/dist/cli-capabilities.manifest.json') as {
      request_schemas?: Record<string, Record<string, unknown>>;
    };
    return manifest.request_schemas ?? {};
  } catch {
    return {};
  }
}
const REQUEST_SCHEMAS = loadRequestSchemas();

// Issue #525: tool input schemas are DERIVED from the capability manifest's
// embedded request JSON schemas — the Rust operation definitions are the
// single source of truth. Tools backed by HTTP-only server surfaces (no CLI
// operation) keep hand-written schemas and are listed here so the split is
// explicit.
const HTTP_ONLY_TOOLS = new Set(['gah_info', 'gah_usage_rollup', 'gah_hold', 'gah_controller_activity', 'gah_loop_status']);

const TOOL_TO_OPERATION: Record<string, string> = {
  gah_status: 'status.get',
  gah_quota: 'quota.snapshot',
  gah_doctor: 'doctor.validate',
  gah_report: 'report.generate',
  gah_profiles: 'profile.list',
  gah_work_history: 'ledger.work',
  gah_sync: 'sync.classify',
  gah_ledger_summary: 'ledger.summary',
  gah_ledger_clear_attempts: 'ledger.clear_attempts',
  gah_availability: 'availability.get',
  gah_availability_clear: 'availability.clear',
  gah_hold_set: 'hold.set',
  gah_hold_clear: 'hold.clear',
  gah_events: 'events.list',
  gah_dispatch: 'dispatch.run',
  gah_route_approvals: 'route_approval.list',
  gah_route_approval_grant: 'route_approval.grant',
  gah_route_approval_revoke: 'route_approval.revoke',
};

type JsonSchemaProperty = { type?: string; description?: string; default?: unknown };

/// Translate the manifest's request JSON schema (the small subset this repo
/// authors: objects with string, number, boolean, and string-array properties) into the zod raw
/// shape registerTool expects.
function zodInputSchemaFor(toolName: string): Record<string, z.ZodTypeAny> | undefined {
  if (HTTP_ONLY_TOOLS.has(toolName)) return undefined;
  const operationId = TOOL_TO_OPERATION[toolName];
  if (!operationId) return undefined;
  const schema = REQUEST_SCHEMAS[operationId];
  if (!schema || typeof schema !== 'object') return undefined;
  const shape: Record<string, z.ZodTypeAny> = {};
  const properties = (schema as { properties?: Record<string, JsonSchemaProperty> }).properties ?? {};
  const required = new Set(
    ((schema as { required?: string[] }).required ?? []) as string[]
  );
  for (const [name, property] of Object.entries(properties)) {
    let field: z.ZodTypeAny;
    switch (property.type) {
      case 'array':
        field = z.array(z.string());
        break;
      case 'number':
        field = z.number();
        break;
      case 'boolean':
        field = z.boolean();
        break;
      default:
        field = z.string();
    }
    if (property.description) field = field.describe(property.description);
    if (property.default !== undefined) {
      shape[name] = field.default(property.default);
      continue;
    }
    if (!required.has(name)) field = field.optional();
    shape[name] = field;
  }
  return shape;
}

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
      inputSchema: zodInputSchemaFor('gah_status'),
    },
    ({ profile }) => tool(() => gah.status(profile ?? DEFAULT_PROFILE))
  );

  server.registerTool(
    'gah_quota',
    {
      title: 'GAH quota snapshot',
      description: 'Usage/quota snapshot for a profile over a time window.',
      inputSchema: zodInputSchemaFor('gah_quota'),
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
      inputSchema: zodInputSchemaFor('gah_doctor'),
    },
    ({ profile }) => tool(() => gah.doctor(profile ?? DEFAULT_PROFILE))
  );

  server.registerTool(
    'gah_report',
    {
      title: 'GAH report',
      description: 'Aggregate usage/cost/success-rate report, optionally grouped by backend or model.',
      inputSchema: zodInputSchemaFor('gah_report'),
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
      inputSchema: zodInputSchemaFor('gah_work_history'),
    },
    ({ workId }) => tool(() => gah.workHistory(workId))
  );

  server.registerTool(
    'gah_sync',
    {
      title: 'GAH sync',
      description: 'Classified open (and recently resolved) merge requests/pull requests for a profile.',
      inputSchema: zodInputSchemaFor('gah_sync'),
    },
    ({ profile }) => tool(() => gah.sync(profile ?? DEFAULT_PROFILE))
  );

  server.registerTool(
    'gah_ledger_summary',
    {
      title: 'GAH ledger summary',
      description: 'Aggregate ledger counts (success/fail, by mode/backend/model, token usage) over a window.',
      inputSchema: zodInputSchemaFor('gah_ledger_summary'),
    },
    ({ profile, since, groupBy }) => tool(() => gah.ledgerSummary(profile, since, groupBy))
  );

  server.registerTool(
    'gah_ledger_clear_attempts',
    {
      title: 'Clear ledger attempts',
      description: 'Append a tombstone ledger entry so a stuck work_id becomes dispatchable again.',
      inputSchema: zodInputSchemaFor('gah_ledger_clear_attempts'),
    },
    ({ profile, work_id, dry_run }) => tool(() => gah.ledgerClearAttempts(profile ?? DEFAULT_PROFILE, work_id, dry_run))
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
      inputSchema: zodInputSchemaFor('gah_availability_clear'),
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
      inputSchema: zodInputSchemaFor('gah_hold_set'),
    },
    ({ profile, work_id, reason }) => tool(() => gah.holdSet(profile ?? DEFAULT_PROFILE, work_id, reason))
  );

  server.registerTool(
    'gah_hold_clear',
    {
      title: 'Clear a review hold',
      description: 'Release a previously set review hold on a work_id.',
      inputSchema: zodInputSchemaFor('gah_hold_clear'),
    },
    ({ profile, work_id }) => tool(() => gah.holdClear(profile ?? DEFAULT_PROFILE, work_id))
  );

  server.registerTool(
    'gah_events',
    {
      title: 'GAH events',
      description: 'Recent controller and dispatch events for a profile.',
      inputSchema: zodInputSchemaFor('gah_events'),
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
      inputSchema: zodInputSchemaFor('gah_dispatch'),
    },
    (args) => tool(() => gah.dispatch({ ...args, profile: args.profile ?? DEFAULT_PROFILE }))
  );

  // Issue #525: paid-route approval tools — a manager agent can see stuck
  // approval requests and grant/revoke the exact scope through the same
  // owner-gated mutation API the dashboard uses (confirm carries the exact
  // work item, backend, account, and model).
  server.registerTool(
    'gah_route_approvals',
    {
      title: 'GAH paid-route approvals',
      description:
        'List pending and active paid-route approval requests for a profile (state, exact scope, consumption).',
      inputSchema: zodInputSchemaFor('gah_route_approvals'),
    },
    ({ profile }) => tool(() => gah.routeApprovals(profile ?? DEFAULT_PROFILE))
  );

  for (const action of ['grant', 'revoke'] as const) {
    server.registerTool(
      `gah_route_approval_${action}`,
      {
        title: `GAH paid-route approval ${action}`,
        description:
          action === 'grant'
            ? 'Grant one exact paid backend/model route for one work item. The scope must match the pending request exactly — it cannot be broadened here.'
            : 'Revoke a previously granted paid-route approval for one exact scope.',
        inputSchema: zodInputSchemaFor(`gah_route_approval_${action}`),
      },
      ({ profile, work_id, backend, backend_instance, model }) =>
        tool(() =>
          gah.routeApprovalChange(action, {
            profile,
            work_id,
            backend,
            backend_instance: backend_instance ?? null,
            model: model ?? null
          })
        )
    );
  }

  return server;
}
