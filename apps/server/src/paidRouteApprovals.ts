import { Router } from 'express';
import type { PaidRouteScope } from '@git-agent-harness/contracts';
import { runPaidRouteApprovals, changePaidRouteApproval } from './gahCli.js';
import type { mutationSafety } from './mutationSafety.js';

const text = (value: unknown, max: number): value is string => typeof value === 'string' && value.length > 0 && value.length <= max && value.trim() === value && !/[\x00-\x1f\x7f]/.test(value);
const sameRoute = (left: PaidRouteScope, right: PaidRouteScope) => left.profile === right.profile && left.work_id === right.work_id && left.backend === right.backend && left.backend_instance === right.backend_instance && left.model === right.model;

/** Mutations require owner access and durable replay/audit guards at mount.
 * Only exact scopes from Rust's current projection can be changed here. */
export function paidRouteApprovalsRouter(mutation: ReturnType<typeof mutationSafety>, read = runPaidRouteApprovals, change = changePaidRouteApproval): Router {
  const router = Router();
  router.use((_req, res, next) => { res.setHeader('Cache-Control', 'no-store'); next(); });
  router.get('/', async (req, res) => {
    if (Object.keys(req.query).some(key => key !== 'profile') || !text(req.query.profile, 128)) return res.status(400).json({ error: 'invalid_profile', message: 'Choose a configured profile.' });
    try { return res.json(await read(req.query.profile)); }
    catch { return res.status(502).json({ error: 'approvals_unavailable', message: 'Cannot load paid-route approvals. Check that this node has an up-to-date GAH CLI.' }); }
  });
  for (const action of ['grant', 'revoke'] as const) {
    router.post(`/${action}`, mutation(`route_approval.${action}`), async (req, res) => {
      const body = req.body;
      const allowed = ['profile', 'work_id', 'backend', 'backend_instance', 'model', 'confirm'];
      if (!body || Object.keys(body).some(key => !allowed.includes(key))
        || !text(body.profile, 128) || !text(body.work_id, 512) || !text(body.backend, 128)
        || !(body.backend_instance === null || text(body.backend_instance, 128))
        || !(body.model === null || text(body.model, 512)) || body.confirm !== true) {
        return res.status(400).json({ error: 'invalid_scope', message: 'Confirm the exact work item, backend, account, and model shown in the approval request.' });
      }
      const scope: PaidRouteScope = { profile: body.profile, work_id: body.work_id, backend: body.backend, backend_instance: body.backend_instance, model: body.model };
      try {
        const routes = await read(scope.profile);
        const route = routes.find(candidate => sameRoute(candidate, scope));
        if (!route || (action === 'grant' && !route.requested && !route.approved)) return res.status(409).json({ error: 'approval_scope_changed', message: 'This approval request changed. Refresh before deciding.' });
        if (route.approved !== (action === 'grant')) await change(action, scope);
        return res.json(await read(scope.profile));
      } catch {
        return res.status(502).json({ error: 'approval_outcome_unknown', message: 'The approval may have changed. Refresh its status before taking another action.' });
      }
    });
  }
  return router;
}
