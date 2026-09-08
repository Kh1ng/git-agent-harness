import { Router } from 'express';
import rateLimit from 'express-rate-limit';
import { effectiveGatewayApiKey, effectiveGatewayUrl, gatewayEnabledForProfile } from './gatewaySettingsStore.js';

/** Fixed gateway operations only. Workers send session keys; central owns the gateway credential. */
export function workerMemoryRouter(): Router {
  const router = Router();
  router.use(rateLimit({ windowMs: 60_000, limit: 60, standardHeaders: true, legacyHeaders: false }));
  for (const [operation, required] of Object.entries({
    recall: ['session_key', 'query'],
    capture: ['session_key', 'user_content', 'assistant_content'],
    'session/end': ['session_key'],
  })) {
    router.post(`/${operation}`, async (req, res) => {
      if (!required.every((key) => typeof req.body?.[key] === 'string') || !req.body.session_key.startsWith('gah:')) {
        res.status(400).json({ error: 'A GAH session_key and the gateway operation fields are required.' });
        return;
      }
      if (typeof req.body.profile !== 'string' || !req.body.profile.trim()) {
        res.status(400).json({ error: 'A profile is required for the central memory policy.' });
        return;
      }
      if (!gatewayEnabledForProfile(req.body.profile)) {
        res.json({ code: 0, context: '', memory_count: 0, l0_recorded: 0, scheduler_notified: false });
        return;
      }
      try {
        const token = effectiveGatewayApiKey();
        const response = await fetch(`${effectiveGatewayUrl().replace(/\/$/, '')}/${operation}`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
          body: JSON.stringify(Object.fromEntries(required.map((key) => [key, req.body[key]]))),
          signal: AbortSignal.timeout(5_000),
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
