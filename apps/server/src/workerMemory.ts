import { Router } from 'express';
import rateLimit from 'express-rate-limit';
import { requireOwner } from './authMiddleware.js';
import { runProfileList } from './gahCli.js';
import { sessionKeyForProfile } from './managerChat/memoryGatewayClient.js';
import { effectiveGatewayApiKey, effectiveGatewayUrl, gatewayEnabledForProfile } from './gatewaySettingsStore.js';

/** Fixed gateway operations only. Workers send session keys; central owns the gateway credential. */
export function workerMemoryRouter(): Router {
  const router = Router();
  router.use(rateLimit({ windowMs: 60_000, limit: 60, standardHeaders: true, legacyHeaders: false }));
  for (const [operation, required] of Object.entries({
    recall: ['session_key', 'query'],
    capture: ['session_key', 'user_content', 'assistant_content'],
    'session/end': ['session_key'],
    'memories/list': ['session_key'],
    'memories/delete': ['session_key', 'id'],
    'memories/migrate-english': ['session_key'],
    'profiles/read': ['session_key', 'filename'],
  })) {
    router.post(`/${operation}`, ...(['memories/delete', 'memories/migrate-english'].includes(operation) ? [requireOwner] : []), async (req, res) => {
      if (!required.every((key) => typeof req.body?.[key] === 'string') || !req.body.session_key.startsWith('gah:')) {
        res.status(400).json({ error: 'A GAH session_key and the gateway operation fields are required.' });
        return;
      }
      if (typeof req.body.profile !== 'string' || !req.body.profile.trim()) {
        res.status(400).json({ error: 'A profile is required for the central memory policy.' });
        return;
      }
      if (operation.startsWith('memories/') || operation === 'profiles/read') {
        if (req.body.profile.length > 2048 || /[\x00-\x1f\x7f]/.test(req.body.profile)
          || required.some(key => req.body[key].length > 2048 || /[\x00-\x1f\x7f]/.test(req.body[key]))) {
          res.status(400).json({error:'Invalid memory operation field.'});
          return;
        }
        const profiles = await runProfileList().catch(() => null);
        if (profiles === null) {
          res.status(503).json({error:'Configured memory profiles are unavailable.'});
          return;
        }
        if (!profiles.some(profile => profile.name === req.body.profile)) {
          res.status(400).json({error:'Memory management requires a configured profile.'});
          return;
        }
        if (req.body.session_key !== await sessionKeyForProfile(req.body.profile)) {
          res.status(400).json({error:'Memory management must use the configured profile project.'});
          return;
        }
      }
      if (!gatewayEnabledForProfile(req.body.profile)) {
        res.json({ code: 0, context: '', memory_count: 0, l0_recorded: 0, scheduler_notified: false });
        return;
      }
      try {
        const token = effectiveGatewayApiKey();
        const response = await fetch(`${effectiveGatewayUrl().replace(/\/$/, '')}/${operation}`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-GAH-Caller':'worker-relay', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
          body: JSON.stringify({ ...Object.fromEntries(required.map((key) => [key, req.body[key]])), ...(operation === 'memories/list' ? {limit:req.body.limit, offset:req.body.offset} : {}) }),
          signal: AbortSignal.timeout(operation === 'memories/migrate-english' || operation === 'session/end' ? 300_000 : 5_000),
          redirect: 'error',
        });
        if (!response.ok) {
          res.status(502).json({ error: `Central memory gateway returned ${response.status}.`, degraded: true });
          return;
        }
        res.json(await response.json());
      } catch {
        res.status(502).json({ error: 'Central memory gateway is unavailable.', degraded: true });
      }
    });
  }
  return router;
}
