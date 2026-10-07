/**
 * Release channel feed (issue #1416). Central compares its own version
 * against the channel's latest published release so the dashboard can show
 * "Update available · vX → vY" without anyone running `gah update` by hand.
 *
 * The channel is the prerelease "edge" feed by default (a green merge to
 * main publishes it; no tag needed) or the stable tag feed via
 * GAH_RELEASE_CHANNEL=stable. Results are cached for a few minutes: the
 * dashboard polls this on load, and unbounded GitHub API reads would burn
 * the anonymous rate limit for nothing.
 */
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import {
  COORDINATOR_VERSION,
  compareVersions,
  type ReleaseChannel,
  type ReleaseChannelStatus
} from '@git-agent-harness/contracts';

const exec = promisify(execFile);

const RELEASE_REPO = process.env.GAH_RELEASE_REPO || 'Kh1ng/git-agent-harness';
const CHANNEL: ReleaseChannel = process.env.GAH_RELEASE_CHANNEL === 'stable' ? 'stable' : 'edge';
const CACHE_TTL_MS = 5 * 60_000;
const MAX_NOTES_CHARS = 8_000;
const MANIFEST_ASSET_NAME = 'edge-manifest.json';

interface GithubRelease {
  tag_name: string;
  draft: boolean;
  prerelease: boolean;
  html_url: string;
  published_at: string | null;
  body: string | null;
  assets: { name: string; browser_download_url: string; state: string }[];
}

export interface ReleaseFeedOptions {
  fetch?: typeof fetch;
  now?: () => number;
  /** Overrides GAH_RELEASE_REPO for tests. */
  repo?: string;
  /** Overrides GAH_RELEASE_CHANNEL for tests. */
  channel?: ReleaseChannel;
}

let cache: { at: number; status: ReleaseChannelStatus } | null = null;

export function resetReleaseFeedCache(): void {
  cache = null;
}

/** Best-effort token: env first, then `gh auth token`; anonymous reads are
 * fine (60/hour against a 5-minute cache) so absence is not an error. */
async function githubToken(): Promise<string | null> {
  const fromEnv = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  if (fromEnv) return fromEnv;
  try {
    const { stdout } = await exec('gh', ['auth', 'token'], { timeout: 5_000, maxBuffer: 4096 });
    const token = stdout.trim();
    return token || null;
  } catch {
    return null;
  }
}

/** The channel's latest published release, or null when the channel has no
 * published release yet (legitimate: before the first edge build). */
async function latestChannelRelease(
  fetchFn: typeof fetch,
  repo: string,
  channel: ReleaseChannel
): Promise<GithubRelease | null> {
  const headers: Record<string, string> = {
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28'
  };
  const token = await githubToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  const response = await fetchFn(`https://api.github.com/repos/${repo}/releases?per_page=30`, {
    headers,
    signal: AbortSignal.timeout(15_000)
  });
  if (!response.ok) throw new Error(`Cannot read releases for ${repo} (HTTP ${response.status}).`);
  const releases = (await response.json()) as GithubRelease[];
  const published = releases.filter(
    (release) =>
      !release.draft && (channel === 'edge' ? release.prerelease : !release.prerelease)
  );
  published.sort((a, b) => Date.parse(b.published_at ?? '') - Date.parse(a.published_at ?? ''));
  return published[0] ?? null;
}

/** Prefer the release's own manifest asset over the tag: the tag on the
 * edge channel is a moving "edge" label, the manifest carries the real
 * version. Falls back to stripping the leading v from the tag. */
async function releaseVersion(
  fetchFn: typeof fetch,
  release: GithubRelease
): Promise<string | null> {
  const manifest = (release.assets ?? []).find(
    (asset) => asset.name === MANIFEST_ASSET_NAME && asset.state === 'uploaded'
  );
  if (manifest) {
    try {
      const response = await fetchFn(manifest.browser_download_url, {
        signal: AbortSignal.timeout(10_000)
      });
      if (response.ok) {
        const parsed = (await response.json()) as { version?: unknown };
        if (typeof parsed.version === 'string' && parsed.version) return parsed.version;
      }
    } catch {
      // Fall through to the tag below.
    }
  }
  const tag = release.tag_name.replace(/^v/, '');
  return tag || null;
}

/** The channel status for this server: what is running, what is published,
 * and whether an update is available. Transport failures throw so the HTTP
 * route can answer 502 instead of a silently stale banner. */
export async function getReleaseStatus(
  options: ReleaseFeedOptions = {}
): Promise<ReleaseChannelStatus> {
  const now = options.now ?? Date.now;
  const repo = options.repo ?? RELEASE_REPO;
  const channel = options.channel ?? CHANNEL;
  const fetchFn = options.fetch ?? fetch;
  if (cache && now() - cache.at < CACHE_TTL_MS) return cache.status;

  const release = await latestChannelRelease(fetchFn, repo, channel);
  const status: ReleaseChannelStatus = {
    channel,
    current_version: COORDINATOR_VERSION,
    latest_version: null,
    update_available: false,
    release_url: null,
    published_at: null,
    notes: ''
  };
  if (release) {
    status.latest_version = await releaseVersion(fetchFn, release);
    status.release_url = release.html_url;
    status.published_at = release.published_at;
    status.notes = (release.body ?? '').slice(0, MAX_NOTES_CHARS);
    status.update_available =
      status.latest_version !== null &&
      compareVersions(status.latest_version, COORDINATOR_VERSION) > 0;
  }
  cache = { at: now(), status };
  return status;
}
