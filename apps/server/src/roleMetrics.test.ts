import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { LedgerEntry } from '@git-agent-harness/contracts';
import { bestFit, confidenceFor, roleMetrics, sinceMs, wilsonLow } from './roleMetrics.js';

const NOW = Date.parse('2026-10-05T00:00:00Z');
let clock = 0;
function entry(overrides: Partial<LedgerEntry> & { mode: string; effective_backend: string }): LedgerEntry {
  clock += 60_000;
  return {
    timestamp: new Date(NOW - 86_400_000 + clock).toISOString(), session_id: null, profile: 'gah', display_name: 'GAH', repo_id: 'gah', repo: 'o/r', local_path: '/r', provider: 'github',
    backend: overrides.effective_backend, requested_backend: 'auto', requested_model: null, effective_model: null, routing_reason: null, fallback_used: false, confidence_impact: null,
    human_required: false, target_summary: null, branch: null, session_dir: null, duration_seconds: 100, backend_exit_code: 0, validation_result: null,
    commit_attempted: false, commit_created: false, push_attempted: false, push_succeeded: false, mr_attempted: false, mr_created: false, mr_url: null,
    files_changed: null, insertions: null, deletions: null, error_summary: null, usage: { usage_source: null, total_tokens: null } as never, ...overrides
  } as LedgerEntry;
}
const tokens = (total_tokens: number) => ({ usage_source: 'attempt_aggregate', total_tokens } as never);

test('rates, Wilson bounds and confidence behave at the edges', () => {
  assert.equal(wilsonLow(0, 0), null);
  assert.equal(wilsonLow(2, 2)! < 0.4, true);
  assert.equal(wilsonLow(30, 40)! > 0.55 && wilsonLow(30, 40)! < 0.75, true);
  assert.equal(confidenceFor(4), 'none');
  assert.equal(confidenceFor(5), 'low');
  assert.equal(confidenceFor(15), 'medium');
  assert.equal(confidenceFor(40), 'high');
  assert.equal(sinceMs('7d'), 7 * 86_400_000);
  assert.equal(sinceMs('all'), null);
});

test('harness errors, holds and tombstones never count against a model', () => {
  const report = roleMetrics([
    entry({ mode: 'fix', effective_backend: 'codex', effective_model: 'gpt-6-sol', failure_class: 'harness_error', error_summary: 'node admission deferred' }),
    entry({ mode: 'fix', effective_backend: 'codex', effective_model: 'gpt-6-sol', failure_class: 'harness_error' }),
    entry({ mode: 'review_hold', effective_backend: '' }),
    entry({ mode: 'fix', effective_backend: 'codex', effective_model: 'gpt-6-sol', mr_created: true, validation_result: 'passed', usage: tokens(1000), work_id: '#1' })
  ], { now: NOW });
  assert.equal(report.entries, 4);
  assert.equal(report.skipped, 1);
  assert.equal(report.harness_errors, 2);
  const cell = report.cells.find((candidate) => candidate.role === 'fix')!;
  assert.equal(cell.attempts, 1);
  assert.equal(cell.harness_errors, 2);
  assert.equal(cell.delivered, 1);
  assert.equal(cell.delivered_rate, 1);
  assert.equal(cell.confidence, 'none');
  assert.equal(cell.tokens_per_delivered, 1000);
  // Harness errors share the model's cell even though they carry no usage; "default" names no model.
  const merged = roleMetrics([
    entry({ mode: 'fix', effective_backend: 'codex', effective_model: 'gpt-6-sol', failure_class: 'harness_error' }),
    entry({ mode: 'fix', effective_backend: 'codex', effective_model: 'gpt-6-sol', mr_created: true, usage: { usage_source: 'attempt_aggregate', total_tokens: 5, backend_instance: 'codex-work' } as never }),
    entry({ mode: 'review', effective_backend: 'agy', effective_model: 'default', review_verdict: 'APPROVE' }),
    entry({ mode: 'review', effective_backend: 'agy', effective_model: null, review_verdict: 'APPROVE' }),
    entry({ mode: 'fix', effective_backend: 'auto' })
  ], { now: NOW });
  assert.equal(merged.cells.length, 2);
  assert.equal(merged.skipped, 1);
  assert.deepEqual(merged.cells.map((c) => [c.role, c.backend, c.model, c.backend_instance, c.attempts, c.harness_errors]), [['fix', 'codex', 'gpt-6-sol', 'codex-work', 1, 1], ['review', 'agy', null, null, 2, 0]]);
});

test('delivered means a PR that was not sent back; first review tells acceptance; reviewers are judged by what followed', () => {
  const entries = [
    // Gemini fixes #10: PR created, reviewed NEEDS_FIX by codex, then codex's fix passes.
    entry({ mode: 'fix', effective_backend: 'agy', effective_model: 'Gemini', work_id: '#10', mr_created: true, validation_result: 'passed', review_verdict: 'NEEDS_FIX', usage: tokens(300) }),
    entry({ mode: 'review', effective_backend: 'codex', effective_model: 'gpt-6-sol', work_id: '#10', review_verdict: 'NEEDS_FIX', validation_result: 'NEEDS_FIX', usage: tokens(50), review_blocking_findings: ['a', 'b'] } as never),
    entry({ mode: 'fix', effective_backend: 'codex', effective_model: 'gpt-6-sol', work_id: '#10', mr_created: true, validation_result: 'passed', review_verdict: 'APPROVE', usage: tokens(400) }),
    // Codex reviews #11 APPROVE, but a fix was needed afterwards: overturned.
    entry({ mode: 'review', effective_backend: 'codex', effective_model: 'gpt-6-sol', work_id: '#11', review_verdict: 'APPROVE', usage: tokens(40) }),
    entry({ mode: 'fix', effective_backend: 'claude', effective_model: 'sonnet', work_id: '#11', mr_created: true, validation_result: 'failed', usage: tokens(900) }),
    // Validation never ran: no validation sample, still an attempt, not delivered.
    entry({ mode: 'fix', effective_backend: 'claude', effective_model: 'sonnet', work_id: '#12', validation_result: 'not_run_no_changes', failure_class: 'agent_no_progress' })
  ];
  const report = roleMetrics(entries, { now: NOW });
  const gemini = report.cells.find((cell) => cell.role === 'fix' && cell.backend === 'agy')!;
  assert.equal(gemini.attempts, 1);
  assert.equal(gemini.delivered, 0);
  assert.equal(gemini.reviewed, 1);
  assert.equal(gemini.first_review_acceptance, 0);
  assert.equal(gemini.validation_pass_rate, 1);
  const codexFix = report.cells.find((cell) => cell.role === 'fix' && cell.backend === 'codex')!;
  assert.equal(codexFix.delivered, 1);
  assert.equal(codexFix.first_review_acceptance, 1);
  const codexReview = report.cells.find((cell) => cell.role === 'review' && cell.backend === 'codex')!;
  assert.equal(codexReview.attempts, 2);
  assert.deepEqual(codexReview.review_verdicts, [['NEEDS_FIX', 1], ['APPROVE', 1]]);
  assert.equal(codexReview.blocking_findings_per_review, 1);
  assert.equal(codexReview.verdicts_vindicated, 1);
  assert.equal(codexReview.verdicts_overturned, 1);
  const sonnet = report.cells.find((cell) => cell.role === 'fix' && cell.backend === 'claude')!;
  assert.equal(sonnet.attempts, 2);
  assert.equal(sonnet.validation_ran, 1);
  assert.equal(sonnet.validation_pass_rate, 0);
  assert.equal(sonnet.delivered, 1);
  // Mixed measurement: tokens per attempt only over measured ones, tokens per delivered needs every attempt measured.
  assert.equal(sonnet.tokens_per_attempt, 900);
  assert.equal(sonnet.tokens_per_delivered, null);

  const fit = report.best_fit.find((role) => role.role === 'fix')!;
  assert.equal(fit.ranking[0].backend, 'codex');
  assert.equal(fit.ranking.every((candidate) => candidate.attempts > 0), true);
});

test('bestFit prefers a steady record over a lucky tiny one, then cheaper tokens', () => {
  const cells = roleMetrics([
    ...Array.from({ length: 40 }, (_, index) => entry({ mode: 'fix', effective_backend: 'codex', effective_model: 'a', work_id: `#${index}`, mr_created: index < 30, usage: tokens(1000) })),
    ...Array.from({ length: 2 }, (_, index) => entry({ mode: 'fix', effective_backend: 'agy', effective_model: 'b', work_id: `#b${index}`, mr_created: true, usage: tokens(100) }))
  ], { now: NOW }).cells;
  const [first, second] = bestFit(cells)[0].ranking;
  // 2 for 2 has a higher raw rate than 30 for 40 but too few attempts to rank first.
  assert.equal(first.backend, 'codex');
  assert.equal(first.confidence, 'high');
  assert.equal(second.confidence, 'none');
});

test('the window and profile filters apply', () => {
  const old = entry({ mode: 'fix', effective_backend: 'codex', mr_created: true });
  old.timestamp = new Date(NOW - 10 * 86_400_000).toISOString();
  const other = entry({ mode: 'fix', effective_backend: 'codex', mr_created: true, profile: 'other' });
  const report = roleMetrics([old, other, entry({ mode: 'fix', effective_backend: 'codex', mr_created: true })], { since: '7d', profile: 'gah', now: NOW });
  assert.equal(report.entries, 1);
});

test('a configured alias is reported as the model the backend actually ran', () => {
  const report = roleMetrics([
    entry({ mode: 'fix', effective_backend: 'claude', effective_model: 'sonnet', failure_class: 'harness_error' }),
    entry({ mode: 'fix', effective_backend: 'claude', effective_model: 'sonnet', mr_created: true }),
    entry({ mode: 'fix', effective_backend: 'claude', effective_model: 'sonnet', mr_created: true, usage: { usage_source: 'x', total_tokens: 5, actual_model: 'claude-sonnet-5-5' } as never }),
    entry({ mode: 'fix', effective_backend: 'codex', effective_model: 'gpt-6-sol', usage: { usage_source: 'x', total_tokens: 5, actual_model: 'gpt-6-sol' } as never })
  ], { now: NOW });
  assert.deepEqual(report.model_aliases, [{ backend: 'claude', alias: 'sonnet', model: 'claude-sonnet-5-5' }]);
  // One cell for the model, whether an entry named the alias or the real thing.
  const claude = report.cells.filter((cell) => cell.backend === 'claude');
  assert.deepEqual(claude.map((cell) => [cell.model, cell.attempts, cell.harness_errors]), [['claude-sonnet-5-5', 2, 1]]);
});
