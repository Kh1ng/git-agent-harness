import express, { Router } from 'express';
import { runConfigShowProfile, runBackendInstanceAdd, runBackendInstanceAuth, runBackendInstanceLabel, runBackendInstanceToggle } from './gahCli.js';
import type { mutationSafety } from './mutationSafety.js';

const text = (value: unknown, max: number): value is string => typeof value === 'string' && value.length > 0 && value.length <= max && value.trim() === value && !/[\x00-\x1f\x7f]/.test(value);

/** Issue #822: per-profile backend-instance enable/disable controls.
 * Reads come from Rust's config projection; mutations shell out to the fixed
 * `gah config set-backend-instance-enabled` command (the CLI owns the config
 * write path, validation, and merged-entry semantics) under the same
 * owner-gated mutation audit/replay guards as every other mutating route. */
export function backendInstancesRouter(
  mutation: ReturnType<typeof mutationSafety>,
  read = runConfigShowProfile,
  toggle = runBackendInstanceToggle,
  add = runBackendInstanceAdd,
  label = runBackendInstanceLabel,
  testAuth = runBackendInstanceAuth
): Router {
  const router = Router();
  router.use((_req, res, next) => { res.setHeader('Cache-Control', 'no-store'); next(); });
  const respondWithInstances = async (res: express.Response, profile: string) => {
    const summary = await read(profile);
    const backend_instances = await Promise.all(summary.backend_instances.map(async (instance) => {
      if (!['codex', 'claude'].includes(instance.runner_kind)) return instance;
      const auth = await testAuth(profile, instance.backend_instance);
      return { ...instance, auth_ready: auth.auth_ready, healthy: auth.auth_ready };
    }));
    return res.json({ profile, backend_instances });
  };
  router.get('/', async (req, res) => {
    if (Object.keys(req.query).some(key => key !== 'profile') || !text(req.query.profile, 128)) {
      return res.status(400).json({ error: 'invalid_profile', message: 'Choose a configured profile.' });
    }
    try { return await respondWithInstances(res, req.query.profile); }
    catch { return res.status(502).json({ error: 'backend_instances_unavailable', message: 'Cannot load backend instances. Check that this node has an up-to-date GAH CLI.' }); }
  });
  for (const action of ['enable', 'disable'] as const) {
    router.post(`/${action}`, mutation('backend_instance.set_enabled'), async (req, res) => {
      const body = req.body;
      const allowed = ['profile', 'instance'];
      if (!body || Object.keys(body).some(key => !allowed.includes(key))
        || !text(body.profile, 128) || !text(body.instance, 128)) {
        return res.status(400).json({ error: 'invalid_instance', message: 'Name both the profile and the backend instance to change.' });
      }
      try {
        // Read-validate first so a stale UI name gets a 409 instead of a CLI error.
        const summary = await read(body.profile);
        const instance = summary.backend_instances.find(candidate => candidate.backend_instance === body.instance);
        if (!instance) return res.status(409).json({ error: 'instance_changed', message: 'That backend instance no longer exists. Refresh before retrying.' });
        if (instance.enabled === (action === 'enable')) return await respondWithInstances(res, body.profile);
        await toggle(body.profile, body.instance, action === 'enable');
        return await respondWithInstances(res, body.profile);
      } catch {
        return res.status(502).json({ error: 'backend_instance_outcome_unknown', message: 'The toggle may not have applied. Refresh the instance state before retrying.' });
      }
    });
  }
  router.post('/add', mutation('backend_instance.configure'), async (req, res) => {
    const body = req.body;
    const allowed = ['profile', 'instance', 'runnerKind', 'accountLabel'];
    if (!body || Object.keys(body).some(key => !allowed.includes(key)) || !text(body.profile, 128)
      || !text(body.instance, 128) || !text(body.accountLabel, 128)
      || !['codex', 'claude'].includes(body.runnerKind)) {
      return res.status(400).json({ error: 'invalid_instance', message: 'Name the profile, account, instance, and Codex or Claude provider.' });
    }
    try {
      await add(body.profile, body.instance, body.runnerKind, body.accountLabel);
      return await respondWithInstances(res, body.profile);
    } catch {
      return res.status(502).json({ error: 'backend_instance_outcome_unknown', message: 'The account may not have been added. Refresh before retrying.' });
    }
  });
  router.post('/label', mutation('backend_instance.configure'), async (req, res) => {
    const body = req.body;
    const allowed = ['profile', 'instance', 'accountLabel'];
    if (!body || Object.keys(body).some(key => !allowed.includes(key)) || !text(body.profile, 128)
      || !text(body.instance, 128) || !text(body.accountLabel, 128)) {
      return res.status(400).json({ error: 'invalid_instance', message: 'Name the profile, instance, and account label.' });
    }
    try {
      await label(body.profile, body.instance, body.accountLabel);
      return await respondWithInstances(res, body.profile);
    } catch {
      return res.status(502).json({ error: 'backend_instance_outcome_unknown', message: 'The label may not have changed. Refresh before retrying.' });
    }
  });
  return router;
}
