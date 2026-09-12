import { Router } from 'express';
import type { ExternalApprovalDecisionScope, ExternalApprovalScope } from '@git-agent-harness/contracts';
import { changeExternalApproval, runExternalApprovals } from './gahCli.js';
import type { mutationSafety } from './mutationSafety.js';

const text = (value: unknown, max: number): value is string => typeof value === 'string' && value.length > 0 && value.length <= max && value.trim() === value && !/[\x00-\x1f\x7f]/.test(value);
const sameScope = (left: ExternalApprovalScope, right: ExternalApprovalDecisionScope) => left.profile === right.profile && left.work_id === right.work_id && left.credential_label === right.credential_label && left.operation_kind === right.operation_kind;

/** Owner decisions are limited to an exact scope projected by Rust. Grant
 * omits all bounds so the CLI inherits, and therefore cannot widen, the request. */
export function externalApprovalsRouter(mutation: ReturnType<typeof mutationSafety>, read = runExternalApprovals, change = changeExternalApproval): Router {
  const router = Router();
  router.use((_req, res, next) => { res.setHeader('Cache-Control', 'no-store'); next(); });
  router.get('/', async (req, res) => {
    if (Object.keys(req.query).some(key => key !== 'profile') || !text(req.query.profile, 128)) return res.status(400).json({ error: 'invalid_profile', message: 'Choose a configured profile.' });
    try { return res.json(await read(req.query.profile)); }
    catch { return res.status(502).json({ error: 'approvals_unavailable', message: 'Cannot load external approvals. Check that this node has an up-to-date GAH CLI.' }); }
  });
  for (const action of ['grant', 'deny', 'revoke'] as const) {
    router.post(`/${action}`, mutation(`external_approval.${action}`), async (req, res) => {
      const body = req.body;
      const allowed = ['profile', 'work_id', 'credential_label', 'operation_kind', 'confirm'];
      if (!body || Object.keys(body).some(key => !allowed.includes(key))
        || !text(body.profile, 128) || !text(body.work_id, 512)
        || !text(body.credential_label, 128) || !text(body.operation_kind, 128)
        || body.confirm !== true) {
        return res.status(400).json({ error: 'invalid_scope', message: 'Confirm the exact work item, credential, and operation shown in the approval request.' });
      }
      const scope: ExternalApprovalDecisionScope = { profile: body.profile, work_id: body.work_id, credential_label: body.credential_label, operation_kind: body.operation_kind };
      try {
        const approvals = await read(scope.profile);
        const approval = approvals.find(candidate => sameScope(candidate, scope));
        const valid = approval && (action === 'grant' || action === 'deny' ? approval.state === 'requested' : approval.active);
        if (!valid) return res.status(409).json({ error: 'approval_scope_changed', message: 'This approval request changed. Refresh before deciding.' });
        await change(action, scope);
        return res.json(await read(scope.profile));
      } catch {
        return res.status(502).json({ error: 'approval_outcome_unknown', message: 'The approval may have changed. Refresh its status before taking another action.' });
      }
    });
  }
  return router;
}
