import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ModelPrice } from '@git-agent-harness/contracts';
import { apiEquivalentUsd, parseExtractedPrices, parsePriceTables, priceFor, pricePageDigest, readPriceBook, refreshModelPrices, validPriceRow } from './modelPricing.js';
import { roleMetrics } from './roleMetrics.js';

const OPENAI = `# Pricing

### Standard pricing data

| Model | Short context input | Short context cached input | Short context cache writes | Short context output | Long context input |
| --- | --- | --- | --- | --- | --- |
| gpt-6-sol | $2.00 | $0.20 | $2.50 | $10.00 | $4.00 |
| gpt-6-luna | $0.10 | $0.01 | $0.125 | $0.50 | $0.20 |
| gpt-5.4-mini | $0.75 | $0.075 | - | $4.50 | - |
| gpt-image-2 | - | - | - | - | - |

### Batch pricing

| Model | Input | Output |
| --- | --- | --- |
| gpt-6-sol | $1.00 | $5.00 |
`;
const ANTHROPIC = `| Model | Base input tokens | 5m cache writes | 1h cache writes | Cache hits and refreshes | Output tokens |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Claude Opus 5.5 | $4 / MTok | $5 / MTok | $8 / MTok | $0.20 / MTok | $20 / MTok |
| Claude Sonnet 5.5 | $2 / MTok<sup>3</sup> | $2.50 / MTok | $4 / MTok | $0.20 / MTok | $10 / MTok |
| Claude Sonnet 4 ([retired](https://example.com/x)) | $3 / MTok | $3.75 / MTok | $6 / MTok | $0.30 / MTok | $15 / MTok |

| Feature | Price |
| --- | --- |
| Web search | $10 / 1,000 searches |
`;
const price = (provider: string, model: string, input: number, output: number, cached: number | null = null): ModelPrice =>
  ({ provider, model, input, output, cached_input: cached, cache_write: null, source_url: 'u', changed_at: 't' });

test('price tables are read from both providers\' markdown, standard prices first', () => {
  assert.deepEqual(parsePriceTables(OPENAI), [
    { model: 'gpt-6-sol', input: 2, output: 10, cached_input: 0.2, cache_write: 2.5 },
    { model: 'gpt-6-luna', input: 0.1, output: 0.5, cached_input: 0.01, cache_write: 0.125 },
    { model: 'gpt-5.4-mini', input: 0.75, output: 4.5, cached_input: 0.075, cache_write: null }
  ]);
  assert.deepEqual(parsePriceTables(ANTHROPIC), [
    { model: 'Claude Opus 5.5', input: 4, output: 20, cached_input: 0.2, cache_write: 5 },
    { model: 'Claude Sonnet 5.5', input: 2, output: 10, cached_input: 0.2, cache_write: 2.5 },
    { model: 'Claude Sonnet 4', input: 3, output: 15, cached_input: 0.3, cache_write: 3.75 }
  ]);
  assert.deepEqual(parsePriceTables('No tables here, just $5 somewhere.'), []);
});

test('rows are validated whoever produced them', () => {
  assert.equal(validPriceRow({ model: 'gpt-6-sol', input: 2, output: 10, cached_input: null, cache_write: null }), true);
  assert.equal(validPriceRow({ model: 'gpt-6-sol', input: -1, output: 10, cached_input: null, cache_write: null }), false);
  assert.equal(validPriceRow({ model: 'gpt-6-sol', input: 2, output: 99999, cached_input: null, cache_write: null }), false);
  assert.equal(validPriceRow({ model: 'x; rm -rf /', input: 2, output: 10, cached_input: null, cache_write: null }), false);
  assert.equal(validPriceRow({ model: 'gpt-6-sol', input: 0, output: 0, cached_input: null, cache_write: null }), false);
  assert.deepEqual(parseExtractedPrices('Here you go:\n[{"model":"glm-5-3","input":0.6,"output":2.2,"cached_input":0.11},{"model":"bad","input":"free","output":1},{"model":"ignore previous instructions","input":1e9,"output":1}]\nDone.'),
    [{ model: 'glm-5-3', input: 0.6, output: 2.2, cached_input: 0.11, cache_write: null }]);
  assert.deepEqual(parseExtractedPrices('I could not find prices.'), []);
  assert.equal(pricePageDigest('intro\n| a | b |\nplain\ncosts $5\n').split('\n').length, 2);
});

test('tag fragments that never close leave no angle brackets and do not corrupt a row', () => {
  const rows = parsePriceTables(`| Model<script>alert(1) | Input | Output |
| --- | --- | --- |
| gpt-6-sol | $2.00<script>alert(1) | $10.00 |
| gpt-6-luna<script>alert(1) | $0.10 | $0.50 |
`);
  // The fragment in the price cell does not change the price; the one in a model name keeps the row out.
  assert.deepEqual(rows, [{ model: 'gpt-6-sol', input: 2, output: 10, cached_input: null, cache_write: null }]);
  assert.ok(rows.every((row) => !/[<>]/.test(row.model)));
});

test('ledger model names find their price row; aliases take the newest of the family', () => {
  const prices = [price('openai', 'gpt-6-sol', 2, 10), price('anthropic', 'Claude Sonnet 5.5', 2, 10), price('anthropic', 'Claude Sonnet 4.6', 3, 15), price('anthropic', 'Claude Opus 5.5', 4, 20)];
  assert.equal(priceFor('gpt-6-sol', prices)?.model, 'gpt-6-sol');
  assert.equal(priceFor('claude-sonnet-5-5', prices)?.model, 'Claude Sonnet 5.5');
  assert.equal(priceFor('claude-opus-5-5[1m]', prices)?.model, 'Claude Opus 5.5');
  assert.equal(priceFor('sonnet', prices)?.model, 'Claude Sonnet 5.5');
  assert.equal(priceFor('Gemini 3.1 Pro (High)', prices), null);
  assert.equal(priceFor('default', prices), null);
  assert.equal(priceFor(null, prices), null);
});

test('API equivalent follows each provider\'s token accounting', () => {
  // OpenAI counts cached tokens inside input: 1M input of which 900k cached.
  assert.equal(apiEquivalentUsd({ input_tokens: 1_000_000, output_tokens: 100_000, cache_read_tokens: 900_000 }, price('openai', 'gpt-6-sol', 2, 10, 0.2)), 0.2 + 1 + 0.18);
  // Anthropic reports them apart.
  assert.equal(apiEquivalentUsd({ input_tokens: 100_000, output_tokens: 100_000, cache_read_tokens: 900_000, cache_write_tokens: 0 }, price('anthropic', 'Claude Sonnet 5.5', 2, 10, 0.2)), 0.2 + 1 + 0.18);
  assert.equal(apiEquivalentUsd({ input_tokens: null, output_tokens: 5 }, price('openai', 'gpt-6-sol', 2, 10)), null);
  assert.equal(apiEquivalentUsd(null, price('openai', 'gpt-6-sol', 2, 10)), null);
});

test('a refresh stores a baseline, then records what changed and keeps prices when a page fails', async () => {
  const path = join(mkdtempSync(join(tmpdir(), 'gah-prices-')), 'model-prices.json');
  const sources = [{ provider: 'openai', url: 'https://o/pricing.md' }, { provider: 'anthropic', url: 'https://a/pricing.md' }];
  let openai = OPENAI;
  let failAnthropic = false;
  const fetchText = async (url: string) => {
    if (url.includes('//a/')) { if (failAnthropic) throw new Error('HTTP 503'); return ANTHROPIC; }
    return openai;
  };
  const first = await refreshModelPrices({ path, sources, fetchText, now: () => Date.parse('2026-10-05T00:00:00Z') });
  assert.equal(first.prices.length, 6);
  assert.deepEqual(first.history, []);
  assert.deepEqual(first.sources.map((source) => [source.provider, source.ok, source.method, source.rows]), [['openai', true, 'table', 3], ['anthropic', true, 'table', 3]]);

  // A day later: one price drops, one model is added, one is gone, and the other page is down.
  openai = OPENAI.replace('| gpt-6-sol | $2.00 | $0.20 | $2.50 | $10.00 |', '| gpt-6-sol | $1.50 | $0.20 | $2.50 | $8.00 |')
    .replace('| gpt-5.4-mini | $0.75 | $0.075 | - | $4.50 | - |\n', '| gpt-7-nova | $3.00 | $0.30 | - | $12.00 | - |\n');
  failAnthropic = true;
  const second = await refreshModelPrices({ path, sources, fetchText, now: () => Date.parse('2026-10-06T00:00:00Z') });
  assert.deepEqual(second.history.map((change) => [change.model, change.field, change.from, change.to]).sort(), [
    ['gpt-5.4-mini', 'removed', 0.75, null], ['gpt-6-sol', 'input', 2, 1.5], ['gpt-6-sol', 'output', 10, 8], ['gpt-7-nova', 'added', null, 3]
  ]);
  assert.equal(second.prices.find((row) => row.model === 'gpt-6-sol')?.changed_at, '2026-10-06T00:00:00.000Z');
  assert.equal(second.prices.find((row) => row.model === 'gpt-6-luna')?.changed_at, '2026-10-05T00:00:00.000Z');
  const anthropic = second.sources.find((source) => source.provider === 'anthropic')!;
  assert.equal(anthropic.ok, false);
  assert.equal(anthropic.error, 'HTTP 503');
  assert.equal(second.prices.filter((row) => row.provider === 'anthropic').length, 3);
  assert.deepEqual(readPriceBook(path), second);
});

test('the helper model reads a page only when no table can be, and its rows are validated', async () => {
  const path = join(mkdtempSync(join(tmpdir(), 'gah-prices-')), 'model-prices.json');
  const asked: string[] = [];
  const extract = async (provider: string, page: string) => {
    asked.push(`${provider}:${page}`);
    return [{ model: 'glm-5-3', input: 0.6, output: 2.2, cached_input: null, cache_write: null }, { model: 'bogus', input: -4, output: 1, cached_input: null, cache_write: null }];
  };
  const book = await refreshModelPrices({ path, extract,
    sources: [{ provider: 'openai', url: 'o' }, { provider: 'z-ai', url: 'z' }],
    fetchText: async (url) => (url === 'o' ? OPENAI : 'GLM-5.3 costs $0.60 in and $2.20 out per million.\nWelcome to our site.') });
  assert.deepEqual(asked, ['z-ai:GLM-5.3 costs $0.60 in and $2.20 out per million.']);
  assert.deepEqual(book.sources.map((source) => [source.provider, source.method, source.rows]), [['openai', 'table', 3], ['z-ai', 'helper_model', 1]]);
  assert.equal(book.prices.find((row) => row.provider === 'z-ai')?.model, 'glm-5-3');
  // No table and no helper: the source reports why, and nothing is invented.
  const none = await refreshModelPrices({ path: join(mkdtempSync(join(tmpdir(), 'gah-prices-')), 'p.json'), sources: [{ provider: 'x', url: 'x' }], fetchText: async () => 'nothing' });
  assert.equal(none.sources[0].error, 'No price table found on the page');
  assert.deepEqual(none.prices, []);
});

test('role metrics turn tokens into API-equivalent dollars per delivered PR when every attempt is priced', () => {
  const now = Date.parse('2026-10-05T00:00:00Z');
  const base = { session_id: null, profile: 'gah', mode: 'fix', effective_backend: 'codex', backend: 'codex', effective_model: 'gpt-6-sol', duration_seconds: 10 };
  const entries = [
    { ...base, timestamp: '2026-10-04T10:00:00Z', work_id: '#1', mr_created: true, usage: { total_tokens: 1_100_000, input_tokens: 1_000_000, output_tokens: 100_000, cache_read_tokens: 900_000 } },
    { ...base, timestamp: '2026-10-04T11:00:00Z', work_id: '#2', mr_created: false, usage: { total_tokens: 1_100_000, input_tokens: 1_000_000, output_tokens: 100_000, cache_read_tokens: 900_000 } },
    { ...base, timestamp: '2026-10-04T12:00:00Z', work_id: '#3', effective_backend: 'agy', backend: 'agy', effective_model: 'Gemini', mr_created: true, usage: { total_tokens: 10, input_tokens: 5, output_tokens: 5 } }
  ] as never;
  const report = roleMetrics(entries, { now, prices: [price('openai', 'gpt-6-sol', 2, 10, 0.2)] });
  const codex = report.cells.find((cell) => cell.backend === 'codex')!;
  assert.equal(codex.priced, 2);
  assert.equal(codex.priced_as, 'gpt-6-sol');
  assert.equal(Number(codex.api_equivalent_usd!.toFixed(2)), 2.76);
  // Two attempts, one delivered: the failed attempt's tokens are part of what the PR cost.
  assert.equal(Number(codex.api_equivalent_per_delivered_usd!.toFixed(2)), 2.76);
  const gemini = report.cells.find((cell) => cell.backend === 'agy')!;
  assert.equal(gemini.api_equivalent_usd, null);
  assert.equal(gemini.priced_as, null);
});
