import assert from 'node:assert/strict';
import { test } from 'node:test';
import { getReleaseStatus, resetReleaseFeedCache } from './releaseFeed.js';
import { COORDINATOR_VERSION } from '@git-agent-harness/contracts';

interface ReleaseFixture {
  tag_name: string;
  prerelease?: boolean;
  draft?: boolean;
  published_at?: string;
  body?: string;
  /** When set, the release carries an edge-manifest.json asset naming this version. */
  manifestVersion?: string;
}

function jsonResponse(value: unknown, ok = true): Response {
  return new Response(JSON.stringify(value), {
    status: ok ? 200 : 500,
    headers: { 'content-type': 'application/json' }
  });
}

/** A fetch fake serving one releases list plus the manifests the listed
 * releases link to. */
function fetchWithReleases(releases: ReleaseFixture[]): typeof fetch {
  const asGithub = (release: ReleaseFixture) => ({
    tag_name: release.tag_name,
    prerelease: release.prerelease === true,
    draft: release.draft === true,
    html_url: `https://github.com/Kh1ng/git-agent-harness/releases/tag/${release.tag_name}`,
    published_at: release.published_at ?? null,
    body: release.body ?? '',
    assets: release.manifestVersion
      ? [{
          name: 'edge-manifest.json',
          browser_download_url: 'https://github.com/Kh1ng/git-agent-harness/releases/download/edge/edge-manifest.json',
          state: 'uploaded'
        }]
      : []
  });
  const listed = releases.map(asGithub);
  return (async (input: string | URL) => {
    const url = String(input);
    if (url.includes('/releases?')) return jsonResponse(listed);
    if (url.endsWith('/edge-manifest.json')) {
      // The chosen release's manifest: serve the fixture's version.
      const version = releases.find((release) => release.manifestVersion)?.manifestVersion ?? '0.0.0';
      return jsonResponse({ schema: 1, version, channel: 'edge', assets: [] });
    }
    return jsonResponse({ error: 'unexpected fetch' }, false);
  }) as unknown as typeof fetch;
}

test('getReleaseStatus reports an available edge release newer than this server', async () => {
  resetReleaseFeedCache();
  const status = await getReleaseStatus({
    fetch: fetchWithReleases([
      {
        tag_name: 'edge',
        prerelease: true,
        published_at: '2026-10-05T00:00:00.000Z',
        body: 'Release notes body',
        manifestVersion: '99.0.0'
      }
    ]),
    now: () => 1_000
  });
  assert.equal(status.channel, 'edge');
  assert.equal(status.current_version, COORDINATOR_VERSION);
  assert.equal(status.latest_version, '99.0.0');
  assert.equal(status.update_available, true);
  assert.equal(status.release_url, 'https://github.com/Kh1ng/git-agent-harness/releases/tag/edge');
  assert.equal(status.notes, 'Release notes body');
});

test('getReleaseStatus ignores stable releases and drafts on the edge channel', async () => {
  resetReleaseFeedCache();
  const status = await getReleaseStatus({
    fetch: fetchWithReleases([
      { tag_name: 'v0.1.2', prerelease: false, published_at: '2026-10-04T00:00:00.000Z' },
      { tag_name: 'v0.2.0', prerelease: false, draft: true, published_at: '2026-10-05T00:00:00.000Z' }
    ]),
    now: () => 2_000
  });
  assert.equal(status.latest_version, null);
  assert.equal(status.update_available, false);
});

test('getReleaseStatus picks the newest edge release by publish time', async () => {
  resetReleaseFeedCache();
  const status = await getReleaseStatus({
    fetch: fetchWithReleases([
      { tag_name: 'edge', prerelease: true, published_at: '2026-10-01T00:00:00.000Z', body: 'older' },
      { tag_name: 'edge', prerelease: true, published_at: '2026-10-03T00:00:00.000Z', body: 'newer' }
    ]),
    now: () => 3_000
  });
  assert.equal(status.notes, 'newer');
});

test('getReleaseStatus falls back to the tag when the release has no manifest asset', async () => {
  resetReleaseFeedCache();
  const status = await getReleaseStatus({
    fetch: fetchWithReleases([
      { tag_name: 'v0.0.1', prerelease: true, published_at: '2026-10-05T00:00:00.000Z' }
    ]),
    now: () => 4_000
  });
  assert.equal(status.latest_version, '0.0.1');
  assert.equal(status.update_available, false);
});

test('getReleaseStatus serves a cached answer within the TTL and refetches after it', async () => {
  resetReleaseFeedCache();
  let calls = 0;
  let body = 'first';
  const fetchFn = (async () => {
    calls += 1;
    return jsonResponse([
      {
        tag_name: 'edge',
        prerelease: true,
        draft: false,
        html_url: 'https://github.com/Kh1ng/git-agent-harness/releases/tag/edge',
        published_at: '2026-10-05T00:00:00.000Z',
        body,
        assets: []
      }
    ]);
  }) as unknown as typeof fetch;

  let clock = 0;
  const first = await getReleaseStatus({ fetch: fetchFn, now: () => clock });
  assert.equal(first.notes, 'first');
  clock += 60_000; // one minute: inside the five-minute TTL.
  const cached = await getReleaseStatus({ fetch: fetchFn, now: () => clock });
  assert.equal(cached.notes, 'first');
  assert.equal(calls, 1);

  body = 'second';
  clock += 5 * 60_000; // past the TTL.
  const refreshed = await getReleaseStatus({ fetch: fetchFn, now: () => clock });
  assert.equal(refreshed.notes, 'second');
  assert.equal(calls, 2);
});

test('getReleaseStatus throws on transport failure so the route can answer 502', async () => {
  resetReleaseFeedCache();
  const fetchFn = (async () => {
    throw new Error('network down');
  }) as unknown as typeof fetch;
  await assert.rejects(
    getReleaseStatus({ fetch: fetchFn, now: () => 5_000 }),
    /network down/
  );
});

test('getReleaseStatus truncates very long release notes', async () => {
  resetReleaseFeedCache();
  const notes = 'x'.repeat(20_000);
  const status = await getReleaseStatus({
    fetch: fetchWithReleases([
      { tag_name: 'edge', prerelease: true, published_at: '2026-10-05T00:00:00.000Z', body: notes }
    ]),
    now: () => 6_000
  });
  assert.equal(status.notes.length, 8_000);
});
