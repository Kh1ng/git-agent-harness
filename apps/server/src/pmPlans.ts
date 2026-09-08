import { Router } from 'express';
import rateLimit from 'express-rate-limit';
import { runPmPlanCommand, type PmPlanCommand } from './gahCli.js';

const validId = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(value) && value !== '.' && value !== '..';

/** Auth is applied at mount. This adapter accepts IDs only; Rust resolves configured
 * profile artifacts, holds the publication lock, and applies provider policy. */
export function pmPlansRouter(run = runPmPlanCommand): Router {
  const router = Router();
  router.use((_req, res, next) => { res.setHeader('Cache-Control', 'no-store'); next(); });
  router.use(rateLimit({ windowMs: 60_000, limit: 30, standardHeaders: true, legacyHeaders: false,
    message: { schema_version: 1, error: 'rate_limited', message: 'Too many PM plan requests. Retry in a minute.' } }));
  router.all(['/plans', '/plans/:planId', '/plans/:planId/:action'], async (req, res) => {
    const action = req.params.action;
    const operation = req.method === 'GET' && !action ? (req.params.planId ? 'show' : 'list')
      : req.method === 'POST' && (action === 'dry-run' || action === 'publish') ? action : null;
    if (!operation) { res.sendStatus(405); return; }
    const input = req.method === 'GET' ? req.query : req.body;
    const allowed = operation === 'list' ? ['profile', 'limit', 'cursor']
      : operation === 'publish' ? ['profile', 'approve', 'plan_fingerprint'] : ['profile'];
    if (!input || Object.keys(input).some(key => !allowed.includes(key))
      || typeof input.profile !== 'string' || !input.profile.trim() || input.profile.length > 128 || /[\x00-\x1f\x7f]/.test(input.profile)
      || (req.params.planId !== undefined && !validId(req.params.planId))
      || (input.cursor !== undefined && !validId(input.cursor))
      || (input.limit !== undefined && (typeof input.limit !== 'string' || !/^(?:[1-9][0-9]?|100)$/.test(input.limit)))) {
      res.status(400).json({ schema_version: 1, error: 'invalid_request', message: 'Use a configured profile, a session plan ID, and supported parameters only.' }); return;
    }
    if (operation === 'publish' && (input.approve !== true || typeof input.plan_fingerprint !== 'string' || !/^[a-f0-9]{64}$/.test(input.plan_fingerprint))) {
      res.status(400).json({ schema_version: 1, error: 'approval_required', message: 'Publication requires approve=true and the reviewed plan_fingerprint.' }); return;
    }
    const command: PmPlanCommand = { operation, profile: input.profile, planId: req.params.planId,
      cursor: input.cursor, limit: input.limit === undefined ? undefined : Number(input.limit), fingerprint: input.plan_fingerprint };
    try {
      const result = await run(command);
      res.status('success' in result && !result.success ? 409 : 200).json(result);
    } catch {
      // Raw spawn/parse errors can include CLI output and local paths. Rust's
      // operation result carries redacted publication errors and partial state.
      res.status(422).json({ schema_version: 1, error: 'plan_unavailable', message: 'Cannot read this plan or apply this request for the configured profile.' });
    }
  });
  return router;
}
