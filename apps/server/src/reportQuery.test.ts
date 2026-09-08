import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { withFixtureServer } from './fixtureGahHarness.js';

// Keep the expected Clap vocabulary independent from the API's allowed values.
const groupings = ['none', 'backend', 'model', 'difficulty', 'backend-difficulty'];
const fixture = (name: string): unknown => JSON.parse(readFileSync(new URL(`../tests/fixtures/gah/responses/${name}.json`, import.meta.url), 'utf8'));

test('report queries preserve CLI arguments and complete fixture JSON', async (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-report-query-'));
  const log = join(directory, 'argv.jsonl');
  const previousLog = process.env.GAH_FIXTURE_QUERY_LOG;
  process.env.GAH_FIXTURE_QUERY_LOG = log;
  t.after(() => {
    if (previousLog === undefined) delete process.env.GAH_FIXTURE_QUERY_LOG;
    else process.env.GAH_FIXTURE_QUERY_LOG = previousLog;
    rmSync(directory, { recursive: true, force: true });
  });
  const calls = (): string[][] => readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line));

  await withFixtureServer(async (baseUrl) => {
    for (const endpoint of ['report', 'ledger/summary']) {
      const command = endpoint === 'report' ? ['report'] : ['ledger', 'summary'];
      const expected = fixture(endpoint === 'report' ? 'report' : 'ledger-summary');
      for (const groupBy of [undefined, ...groupings]) {
        for (const since of [undefined, '30d', '24h']) {
          await t.test(`${endpoint} group=${groupBy ?? 'default'} since=${since ?? 'default'}`, async () => {
            writeFileSync(log, '');
            const query = new URLSearchParams();
            if (groupBy !== undefined) query.set('groupBy', groupBy);
            if (since !== undefined) {
              query.set('since', since);
              query.set('profile', 'fixture profile');
            }
            const response = await fetch(`${baseUrl}/api/${endpoint}?${query}`);
            assert.equal(response.status, 200);
            assert.deepEqual(await response.json(), expected);
            const args = [...command, '--json', '--since', since ?? '7d'];
            if (endpoint === 'report') args.push('--group-by', groupBy ?? 'backend');
            if (since !== undefined) args.push('--profile', 'fixture profile');
            if (endpoint !== 'report' && groupBy !== undefined) args.push('--group-by', groupBy);
            assert.deepEqual(calls(), [args]);
          });
        }
      }
      await t.test(`${endpoint} rejects unsupported and non-scalar groupings without a CLI call`, async () => {
        for (const query of ['groupBy=unsupported-secret', 'groupBy=', 'groupBy=Backend', 'groupBy=backenddifficulty', 'groupBy=backend&groupBy=model', 'groupBy[key]=model']) {
          writeFileSync(log, '');
          const response = await fetch(`${baseUrl}/api/${endpoint}?${query}`);
          assert.equal(response.status, 400, query);
          assert.deepEqual(await response.json(), {
            error: 'Invalid groupBy',
            message: 'groupBy must be one of: none, backend, model, difficulty, backend-difficulty'
          });
          assert.deepEqual(calls(), []);
        }
      });
    }
    for (const since of [undefined, '14d', '24h']) {
      await t.test(`series preserves ${since ?? 'CLI default'} date window`, async () => {
        writeFileSync(log, '');
        const query = new URLSearchParams();
        if (since !== undefined) {
          query.set('since', since);
          query.set('bucket', 'daily');
          query.set('profile', 'fixture profile');
        }
        const response = await fetch(`${baseUrl}/api/report/series?${query}`);
        assert.equal(response.status, 200);
        assert.deepEqual(await response.json(), fixture('report-series'));
        const args = ['report', '--json', '--series', '--since', since ?? '7d', '--bucket', 'daily'];
        if (since !== undefined) args.push('--profile', 'fixture profile');
        assert.deepEqual(calls(), [args]);
      });
    }
  });
});
