import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { gunzipSync } from 'node:zlib';
import { buildDiagnosticsBundle, parseProcesses } from './diagnostics.js';

test('keeps the largest processes and every server descendant, without arguments', () => {
  const ps = [
    '    1     0   9000  0.0 /sbin/launchd',
    '  100     1  50000  1.0 /Applications/Parasaur.app/Contents/MacOS/parasaur',
    '  200     1   4000  0.5 node',
    '  300   200    100  0.0 /usr/local/bin/claude',
    '  400   300     50 12.5 opencode',
    '  500     1     10  0.0 sleep'
  ].join('\n');
  const result = parseProcesses(ps, 200, 2);
  assert.deepEqual(result.map((p) => p.pid), [100, 1, 200, 300, 400]);
  assert.equal(result[0].cmd, 'parasaur');
  assert.equal(result.at(-1)!.cpu, 12.5);
});

test('bundle carries the rotated and current timeline and skips torn lines', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-diag-'));
  writeFileSync(join(dir, 'memory.1.jsonl'), '{"t":"a"}\n');
  writeFileSync(join(dir, 'memory.jsonl'), '{"t":"b"}\n{"t":');
  const bundle = JSON.parse(gunzipSync(await buildDiagnosticsBundle('worker', dir)).toString());
  assert.equal(bundle.format, 'parasaur-diagnostics/1');
  assert.equal(bundle.node_role, 'worker');
  assert.deepEqual(bundle.timeline.map((s: { t: string }) => s.t), ['a', 'b']);
  assert.ok(bundle.current.server.rss_bytes > 0);
});
