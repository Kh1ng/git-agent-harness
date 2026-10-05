import { expect, test } from '@playwright/test';
import { openUsage } from './helpers/navigation.js';

// Telemetry opens with "Model fit by role": per job kind, how each model
// did, and a best-fit pick per role with a confidence from the sample size.

const cell = (role: string, backend: string, model: string, attempts: number, delivered: number, extra: Record<string, unknown> = {}) => ({
  role, backend, backend_instance: null, model, attempts, harness_errors: 0, delivered, delivered_rate: attempts ? delivered / attempts : null,
  delivered_rate_low: attempts ? Math.max(0, delivered / attempts - 0.2) : null, validation_ran: 0, validation_passed: 0, validation_pass_rate: null,
  reviewed: 0, approved_first_review: 0, first_review_acceptance: null, measured: attempts, total_tokens: attempts * 1_000_000, tokens_per_attempt: 1_000_000,
  tokens_per_delivered: delivered ? (attempts * 1_000_000) / delivered : null, total_cost_usd: null, cost_per_delivered_usd: null, median_duration_seconds: 600,
  review_verdicts: [], blocking_findings_per_review: null, verdicts_vindicated: 0, verdicts_overturned: 0,
  confidence: attempts >= 40 ? 'high' : attempts >= 15 ? 'medium' : attempts >= 5 ? 'low' : 'none', ...extra
});

test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: 'ignoreErrors' }); });

test('Telemetry leads with model fit by role and a best-fit pick with confidence', async ({ page }) => {
  await page.route('**/api/report/roles**', (route) => route.fulfill({ json: { since: '7d', profile: 'fixture', entries: 60, skipped: 2, harness_errors: 20, cells: [
    cell('fix', 'codex', 'gpt-6-sol', 20, 12, { harness_errors: 18, validation_ran: 10, validation_passed: 8, validation_pass_rate: 0.8, reviewed: 10, approved_first_review: 7, first_review_acceptance: 0.7 }),
    cell('fix', 'agy', 'Gemini 3.1 Pro (High)', 2, 2),
    cell('review', 'codex', 'gpt-6-sol', 8, 8, { review_verdicts: [['NEEDS_FIX', 6], ['APPROVE', 2]], blocking_findings_per_review: 1.5, verdicts_vindicated: 4, verdicts_overturned: 1 }),
    cell('improve', 'claude', 'sonnet', 3, 1)
  ], best_fit: [
    { role: 'fix', ranking: [{ backend: 'codex', backend_instance: null, model: 'gpt-6-sol', score: 0.4, attempts: 20, confidence: 'medium' }, { backend: 'agy', backend_instance: null, model: 'Gemini 3.1 Pro (High)', score: 0.34, attempts: 2, confidence: 'none' }] },
    { role: 'improve', ranking: [{ backend: 'claude', backend_instance: null, model: 'sonnet', score: 0.06, attempts: 3, confidence: 'none' }] },
    { role: 'review', ranking: [{ backend: 'codex', backend_instance: null, model: 'gpt-6-sol', score: 0.68, attempts: 8, confidence: 'low' }] }
  ] } }));
  await page.goto('/');
  await openUsage(page, 'Telemetry');
  const card = page.getByRole('region', { name: /Model fit by role/ });
  // It is the first card on the page.
  const firstCard = page.getByRole('main').locator('section').first();
  await expect(firstCard).toContainText('Model fit by role');
  await expect(card).toContainText('60 ledger entries · 20 harness errors · 2 not attempts');

  const best = card.getByRole('list', { name: 'Best fit by role' }).getByRole('listitem');
  await expect(best.filter({ hasText: 'Fix' })).toContainText('Codex gpt-6-sol');
  await expect(best.filter({ hasText: 'Fix' })).toContainText('at least 40% delivered · 20 attempts · medium confidence');
  await expect(best.filter({ hasText: 'Implement' })).toContainText('Too few attempts to say');
  await expect(best.filter({ hasText: 'Implement' })).toContainText('Claude sonnet leads on 3; 5 attempts needed');

  const fix = card.locator('tr[data-role="fix"][data-model="Codex gpt-6-sol"]');
  await expect(fix).toContainText('20+18 harness');
  await expect(fix).toContainText('60% (12/20)');
  await expect(fix).toContainText('80% (8/10)');
  await expect(fix).toContainText('70% (7/10)');
  await expect(fix).toContainText('1.7M');
  await expect(fix).toContainText('10 min');
  await expect(fix).toContainText('medium confidence');
  const review = card.locator('tr[data-role="review"]');
  await expect(review).toContainText('needs fix 6 · approve 2');
  await expect(review).toContainText('4 right · 1 overturned');
  await expect(card.locator('tr[data-role="fix"][data-model="Antigravity Gemini 3.1 Pro (High)"]')).toContainText('too few to judge');
});
