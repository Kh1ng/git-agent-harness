/**
 * Development-only fixtures. Every export is prefixed `DEV_FIXTURE_` so a
 * grep instantly shows every place fake data could leak into a real view,
 * and nothing here is imported by the default (live) data path -- only by
 * the trend chart's explicit integration-gap fallback (TelemetryPage) and,
 * optionally, Storybook-less manual visual QA.
 *
 * These exist because Phase 12/8 asks for representative states to design
 * and screenshot against, and because the daily-trend visualization has no
 * real backend time-series endpoint yet (gah report is a totals-only
 * aggregate) -- rather than inventing values presented as real, the trend
 * chart is built against this typed, clearly-labeled fixture and the
 * integration gap is called out in the UI (see the badge in
 * TelemetryPage.tsx).
 */
import type { TrendPoint } from './components/TrendChart.js';
import type { BackendModelComparison, AvailabilityScope, MergeRequest, AvailableTicket } from '@git-agent-harness/contracts';

function daysAgo(n: number): string {
  const d = new Date();
  d.setUTCDate(d.getUTCDate() - n);
  return d.toISOString().slice(0, 10);
}

/** 14-day synthetic token-usage trend. NOT wired into any live view --
 * see the module doc comment. Values are plausible but invented, which is
 * exactly why this is fixture-only and always shown with a visible badge. */
export const DEV_FIXTURE_TREND_TOKENS: TrendPoint[] = Array.from({ length: 14 }, (_, i) => ({
  date: daysAgo(13 - i),
  value: Math.round(8000 + Math.sin(i / 2) * 3000 + i * 400)
}));

export const DEV_FIXTURE_TREND_COST: TrendPoint[] = Array.from({ length: 14 }, (_, i) => ({
  date: daysAgo(13 - i),
  value: Math.round((1.2 + Math.sin(i / 3) * 0.6 + i * 0.08) * 100) / 100
}));

export const DEV_FIXTURE_TREND_SUCCESS_RATE: TrendPoint[] = Array.from({ length: 14 }, (_, i) => ({
  date: daysAgo(13 - i),
  value: Math.round((0.6 + Math.sin(i / 4) * 0.15 + i * 0.01) * 100) / 100
}));

// ---------------------------------------------------------------------------
// Representative states for manual visual QA / screenshotting (Phase 12).
// Not imported by any page's default render path.
// ---------------------------------------------------------------------------

export const DEV_FIXTURE_REPORT_COMPARISONS: BackendModelComparison[] = [
  {
    backend_or_model: 'claude',
    is_model: false,
    entries: 42,
    attempts: 51,
    validation_pass: 38,
    success_rate: 0.905,
    total_cost_usd: 12.4,
    actual_cost_usd: 12.4,
    estimated_cost_usd: null,
    average_cost_usd: 0.295,
    average_duration_seconds: 184.2,
    input_tokens: 1_240_000,
    output_tokens: 310_000,
    cache_read_tokens: 890_000,
    cache_write_tokens: 42_000,
    total_tokens: 1_550_000,
    requests_count: 42,
    quota_observations: [],
    review_verdict_distribution: [
      ['APPROVE_STRONG', 20],
      ['NEEDS_FIX', 5]
    ]
  },
  {
    backend_or_model: 'codex',
    is_model: false,
    entries: 18,
    attempts: 22,
    validation_pass: 12,
    success_rate: 0.667,
    total_cost_usd: null,
    actual_cost_usd: null,
    estimated_cost_usd: 3.1,
    average_cost_usd: 0.172,
    average_duration_seconds: 240.7,
    input_tokens: 410_000,
    output_tokens: 88_000,
    cache_read_tokens: null,
    cache_write_tokens: null,
    total_tokens: 498_000,
    requests_count: 18,
    quota_observations: [],
    review_verdict_distribution: []
  },
  {
    backend_or_model: 'openhands',
    is_model: false,
    entries: 6,
    attempts: 9,
    validation_pass: 2,
    success_rate: 0.333,
    total_cost_usd: null,
    actual_cost_usd: null,
    estimated_cost_usd: null,
    average_cost_usd: null,
    average_duration_seconds: null,
    input_tokens: null,
    output_tokens: null,
    cache_read_tokens: null,
    cache_write_tokens: null,
    total_tokens: null,
    requests_count: null,
    quota_observations: [],
    review_verdict_distribution: []
  }
];

export const DEV_FIXTURE_AVAILABILITY_FRESH: AvailabilityScope = {
  backend: 'claude',
  model: null,
  quota_pool: 'claude-main',
  eligible_now: true,
  reason: null,
  unavailable_until: null,
  source: 'backend_error',
  last_error_summary: null,
  observed_at: new Date(Date.now() - 8 * 60 * 1000).toISOString(),
  scope: 'quota_pool'
};

export const DEV_FIXTURE_AVAILABILITY_STALE: AvailabilityScope = {
  backend: 'agy',
  model: null,
  quota_pool: 'agy-second',
  eligible_now: true,
  reason: null,
  unavailable_until: null,
  source: 'backend_error',
  last_error_summary: null,
  observed_at: new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString(),
  scope: 'quota_pool'
};

export const DEV_FIXTURE_AVAILABILITY_UNKNOWN: AvailabilityScope = {
  backend: 'grok',
  model: null,
  quota_pool: null,
  eligible_now: true,
  reason: null,
  unavailable_until: null,
  source: null,
  last_error_summary: null,
  observed_at: null,
  scope: null
};

export const DEV_FIXTURE_AVAILABILITY_EXHAUSTED: AvailabilityScope = {
  backend: 'codex',
  model: null,
  quota_pool: 'codex-main',
  eligible_now: false,
  reason: 'quota_exhausted',
  unavailable_until: new Date(Date.now() + 2 * 60 * 60 * 1000 + 14 * 60 * 1000).toISOString(),
  source: 'backend_error',
  last_error_summary: "You've hit your usage limit... try again at 9:01 PM.",
  observed_at: new Date(Date.now() - 5 * 60 * 1000).toISOString(),
  scope: 'backend_wide'
};

export const DEV_FIXTURE_MR_NEEDS_REVIEW: MergeRequest = {
  profile: 'gah',
  branch: 'gah/gah-1783000000',
  work_id: 'TICKET-201',
  id: '201',
  url: 'https://github.com/example/gah/pull/201',
  state: 'OPEN',
  draft: false,
  merge_status: 'CLEAN',
  merged: false,
  ci_passed: true,
  classification: 'NEEDS_REVIEW',
  recommended_action: 'RUN_REVIEW'
};

export const DEV_FIXTURE_TICKET_HUMAN_REQUIRED: AvailableTicket = {
  ticket_path: 'docs/tickets/TICKET-202-flaky-retry.md',
  work_id: 'TICKET-202',
  title: 'Fix flaky retry classification',
  recommended_backend: 'claude',
  recommended_model: 'claude-sonnet-5',
  prior_attempt_count: 2,
  last_failure_class: 'agent_no_progress',
  has_active_mr: false,
  human_required: true
};
