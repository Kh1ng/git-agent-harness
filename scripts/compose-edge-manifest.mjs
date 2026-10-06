#!/usr/bin/env node
// Compose the edge channel's manifest and (when desktop updater fragments
// are present) the Tauri updater feed for the release-edge workflow
// (issue #1416). Run from inside the downloaded-artifacts directory:
//
//   node scripts/compose-edge-manifest.mjs <head-sha>
//
// Expected in the CWD: gah-linux-x86_64, gah-macos-universal, the matching
// gah-mcp-server-* binaries, gah-server-bundle.tar.gz, optional fragment-*.json (one per desktop
// platform, referencing the signed updater asset staged beside it).
// Writes edge-manifest.json and, when fragments exist, latest.json.
import { readFileSync, writeFileSync, existsSync, readdirSync, statSync } from 'node:fs';
import { createHash } from 'node:crypto';

const repo = process.env.GITHUB_REPOSITORY || 'Kh1ng/git-agent-harness';
const headSha = process.argv[2];
if (!headSha || !/^[0-9a-f]{7,40}$/.test(headSha)) {
  throw new Error('usage: compose-edge-manifest.mjs <head-sha>');
}
// The product version lives in the repository root, not among the artifacts.
const version = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).version;
const sha256 = (name) => createHash('sha256').update(readFileSync(name)).digest('hex');

const KINDS = {
  'gah-linux-x86_64': 'cli',
  'gah-macos-universal': 'cli',
  'gah-mcp-server-linux-x86_64': 'mcp-server',
  'gah-mcp-server-macos-universal': 'mcp-server',
  'gah-server-bundle.tar.gz': 'server-bundle'
};
const assets = [];
for (const [name, kind] of Object.entries(KINDS)) {
  if (!existsSync(name)) throw new Error(`missing edge asset: ${name}`);
  assets.push({
    name,
    kind,
    url: `https://github.com/${repo}/releases/download/edge/${name}`,
    sha256: sha256(name),
    size: statSync(name).size
  });
}

const manifest = {
  schema: 1,
  version,
  channel: 'edge',
  commit: headSha,
  published_at: new Date().toISOString(),
  notes_url: `https://github.com/${repo}/releases/tag/edge`,
  assets
};
writeFileSync('edge-manifest.json', `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`edge-manifest.json: ${version} @ ${headSha.slice(0, 10)} (${assets.length} assets)`);

// Desktop updater feed: one fragment per desktop platform, each naming its
// signed asset. Absent fragments mean the signing key is not configured;
// the desktop app then reports "updater not configured" and nothing else
// changes.
const fragments = readdirSync('.').filter((name) => /^fragment-.*\.json$/.test(name));
if (fragments.length > 0) {
  const platforms = {};
  for (const fragmentName of fragments) {
    const fragment = JSON.parse(readFileSync(fragmentName, 'utf8'));
    if (!Array.isArray(fragment.platform_keys) || typeof fragment.asset_name !== 'string' || typeof fragment.signature !== 'string') {
      throw new Error(`invalid updater fragment: ${fragmentName}`);
    }
    if (!existsSync(fragment.asset_name)) throw new Error(`missing updater asset: ${fragment.asset_name}`);
    for (const key of fragment.platform_keys) {
      platforms[key] = {
        signature: fragment.signature,
        url: `https://github.com/${repo}/releases/download/edge/${fragment.asset_name}`
      };
    }
  }
  writeFileSync('latest.json', `${JSON.stringify({
    version,
    notes: `GAH ${version} (edge, commit ${headSha.slice(0, 10)})`,
    pub_date: new Date().toISOString(),
    platforms
  }, null, 2)}\n`);
  console.log(`latest.json: ${Object.keys(platforms).join(', ')}`);
}
