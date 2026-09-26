import assert from 'node:assert/strict';
import { test } from 'node:test';
import { redactTextSecrets } from './redactText.js';

test('redactTextSecrets scrubs every credential shape before text crosses a boundary', () => {
  const cases: Array<[string, string]> = [
    ['token ghp_0123456789abcdefghijklmnop', 'token [REDACTED:GITHUB_TOKEN]'],
    ['token gho_0123456789abcdefghijklmnop', 'token [REDACTED:GITHUB_TOKEN]'],
    ['token ghu_0123456789abcdefghijklmnop', 'token [REDACTED:GITHUB_TOKEN]'],
    ['token ghr_0123456789abcdefghijklmnop', 'token [REDACTED:GITHUB_TOKEN]'],
    ['token ghs_0123456789abcdefghijklmnop', 'token [REDACTED:GITHUB_TOKEN]'],
    ['fine-grained github_pat_0123456789abcdefghijklmnop', 'fine-grained [REDACTED:GITHUB_TOKEN]'],
    ['glpat-0123456789abcdefghijklmnop', '[REDACTED:GITLAB_TOKEN]'],
    ['key sk-0123456789abcdefghijklmnopqrs', 'key [REDACTED:API_KEY]'],
    ['Authorization: Bearer abc123def456', 'Authorization: Bearer [REDACTED:TOKEN]'],
    ['authorization:bearer abc123def456', 'authorization:bearer [REDACTED:TOKEN]'],
    ['clone https://user:secret@github.com/owner/repo.git', 'clone https://[REDACTED:URL_CREDENTIAL]@github.com/owner/repo.git'],
    ['call /v1/x?access_token=abc123&x=1', 'call /v1/x?access_token=[REDACTED:URL_CREDENTIAL]&x=1'],
    ['call /v1/x?api_key=abc123', 'call /v1/x?api_key=[REDACTED:URL_CREDENTIAL]'],
    ['call /v1/x?token=abc123&other=2', 'call /v1/x?token=[REDACTED:URL_CREDENTIAL]&other=2'],
    ['call /v1/x?password=abc123', 'call /v1/x?password=[REDACTED:URL_CREDENTIAL]'],
    ['api_key: abc123', 'api_key: [REDACTED:SECRET]'],
    ['API-KEY=abc123', 'API-KEY=[REDACTED:SECRET]'],
    ['token = abc123', 'token = [REDACTED:SECRET]'],
    ['secret: abc123', 'secret: [REDACTED:SECRET]'],
    ['password: abc123', 'password: [REDACTED:SECRET]'],
  ];
  for (const [input, expected] of cases) {
    assert.equal(redactTextSecrets(input), expected, `input: ${input}`);
  }
});

test('redactTextSecrets leaves ordinary text and short non-credentials intact', () => {
  const prose = 'Look at issue #71, run cargo test, and see https://github.com/owner/repo for details.';
  assert.equal(redactTextSecrets(prose), prose);
  // Prefix-like strings that do not meet the minimum length stay visible.
  assert.equal(redactTextSecrets('sk-short'), 'sk-short');
  assert.equal(redactTextSecrets('ghp_short'), 'ghp_short');
  assert.equal(redactTextSecrets('glpat-short'), 'glpat-short');
  // A keyword without a separator/value is ordinary prose, not a credential.
  assert.equal(redactTextSecrets('the password was rotated'), 'the password was rotated');
});

test('redactTextSecrets redacts a mixed chat message while preserving structure', () => {
  const message = [
    'I generated a deploy token: ghp_0123456789abcdefghijklmnop',
    'Use it via Authorization: Bearer abc123def456',
    'against https://ci:secret@internal.example.com/v1?token=zzz',
    'to close ticket #71.',
  ].join('\n');
  const redacted = redactTextSecrets(message);
  assert.equal(redacted, [
    'I generated a deploy token: [REDACTED:GITHUB_TOKEN]',
    'Use it via Authorization: Bearer [REDACTED:TOKEN]',
    'against https://[REDACTED:URL_CREDENTIAL]@internal.example.com/v1?token=[REDACTED:URL_CREDENTIAL]',
    'to close ticket #71.',
  ].join('\n'));
});
