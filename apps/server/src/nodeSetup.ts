import { Router } from 'express';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const exec = promisify(execFile);
const repoRoot = fileURLToPath(new URL('../../../', import.meta.url));
const repo = 'Kh1ng/git-agent-harness';

function psQuote(value: string): string { return `'${value.replaceAll("'", "''")}'`; }

export function windowsSetupCommand(centralUrl: string, role: string, token: string): string {
  const url = new URL(centralUrl);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
    throw new Error('Enter the central node origin, for example http://192.168.1.10:3773.');
  }
  if (['localhost', '127.0.0.1', '[::1]', '0.0.0.0'].includes(url.hostname) || url.hostname.startsWith('127.')) {
    throw new Error('Use the central node’s LAN or VPN address. Loopback addresses point to the new computer.');
  }
  if (!['desktop', 'worker', 'both'].includes(role)) throw new Error('Choose desktop, worker, or both.');
  if (!token || /[\r\n]/.test(token)) throw new Error('Configure COORDINATOR_TOKEN on the central server first.');
  return `$u=${psQuote(url.origin)};$t=${psQuote(token)};& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing -Headers @{Authorization="Bearer $t"} -Uri "$u/api/settings/nodes/install.ps1").Content)) -CentralUrl $u -CoordinatorToken $t -Role ${psQuote(role)}`;
}

export function isSupportedWindowsInstaller(name: string): boolean {
  const match = /_(\d+)\.(\d+)\.(\d+)_x64-setup\.exe$/.exec(name);
  if (!match) return false;
  const [major, minor, patch] = match.slice(1).map(Number);
  return major > 0 || minor > 1 || (minor === 1 && patch >= 1);
}

async function githubHeaders(): Promise<Record<string, string>> {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN ||
    (await exec('gh', ['auth', 'token'], { timeout: 5000, maxBuffer: 4096 })).stdout.trim();
  if (!token) throw new Error('The central server needs GitHub release read access (GH_TOKEN or gh auth login).');
  return { Authorization: `Bearer ${token}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28' };
}

// Mounted below /api/settings so the existing transport and bearer auth gate applies.
export function nodeSetupRouter(): Router {
  const router = Router();
  router.use((_req, res, next) => { res.setHeader('Cache-Control', 'no-store'); next(); });
  router.post('/command', (req, res) => {
    try {
      if (req.body.role !== 'desktop' && process.env.GAH_ALLOW_INSECURE_HTTP !== '1') {
        return res.status(409).json({ message: 'WSL worker enrollment currently uses trusted LAN transport. Set GAH_ALLOW_INSECURE_HTTP=1 on the central server for a trusted LAN/VPN, or install the desktop only.' });
      }
      const command = windowsSetupCommand(req.body.centralUrl, req.body.role, process.env.COORDINATOR_TOKEN ?? '');
      return res.json({ command });
    } catch (err) {
      return res.status(400).json({ message: err instanceof Error ? err.message : 'Invalid setup request.' });
    }
  });
  for (const [route, file] of [['install.ps1', 'install-windows.ps1'], ['install-wsl.sh', 'install-wsl-worker.sh']]) {
    router.get(`/${route}`, async (_req, res) => {
      try { res.type('text/plain').send(await readFile(resolve(repoRoot, 'scripts', file), 'utf8')); }
      catch { res.status(503).json({ message: 'Installer is missing from this server checkout.' }); }
    });
  }
  router.get('/source.tar.gz', async (_req, res) => {
    try {
      // Export tracked source only: never local credentials, config, or caches.
      const { stdout } = await exec('git', ['archive', '--format=tar.gz', 'HEAD'], { cwd: repoRoot, encoding: 'buffer', maxBuffer: 128 * 1024 * 1024, timeout: 60_000 });
      res.type('application/gzip').send(stdout);
    } catch { res.status(503).json({ message: 'Cannot export the installed source revision. This server needs its Git checkout.' }); }
  });
  router.get('/release/:kind', async (req, res) => {
    const kind = req.params.kind;
    if (!['desktop', 'linux-cli'].includes(kind)) return res.sendStatus(404);
    try {
      const headers = await githubHeaders();
      const releases = await fetch(`https://api.github.com/repos/${repo}/releases?per_page=30`, { headers, signal: AbortSignal.timeout(15_000) });
      if (!releases.ok) throw new Error('Cannot read releases');
      const data = await releases.json() as { draft: boolean; prerelease: boolean; assets: { id: number; name: string; size: number }[] }[];
      const asset = data.filter((release) => !release.draft && !release.prerelease).flatMap((release) => release.assets)
        .find((asset) => kind === 'desktop' ? isSupportedWindowsInstaller(asset.name) : asset.name === 'gah-linux-x86_64');
      if (!asset || asset.size > 128 * 1024 * 1024) throw new Error('Missing or oversized asset');
      const download = await fetch(`https://api.github.com/repos/${repo}/releases/assets/${asset.id}`, { headers: { ...headers, Accept: 'application/octet-stream' }, signal: AbortSignal.timeout(120_000) });
      if (!download.ok) throw new Error('Asset download failed');
      res.type('application/octet-stream').send(Buffer.from(await download.arrayBuffer()));
    } catch {
      res.status(503).json({ message: 'Cannot download the release. Check the central server’s GitHub access and published desktop 0.1.1+ and Linux x64 CLI assets.' });
    }
  });
  return router;
}
