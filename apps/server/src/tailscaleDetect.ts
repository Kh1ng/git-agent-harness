/**
 * Best-effort detection of this host's own Tailscale IPv4 address, for
 * building an "Add a Node" command a genuinely different machine can
 * paste and actually reach (issue #880/#881 follow-up). The gateway's own
 * configured URL is typically loopback-optimized for this server's own
 * calls (http://127.0.0.1:8420) -- useless to a remote machine, which
 * needs this host's real address instead.
 *
 * Never throws: missing `tailscale` binary, not logged in, or malformed
 * output all resolve to `null` so callers degrade to "can't auto-detect,
 * fill in the host yourself" rather than failing the whole request.
 */
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);

export async function detectTailscaleIPv4(): Promise<string | null> {
  try {
    const { stdout } = await execFileAsync('tailscale', ['status', '--json'], { timeout: 3000 });
    const status = JSON.parse(stdout) as { Self?: { TailscaleIPs?: string[] } };
    const ips = status.Self?.TailscaleIPs ?? [];
    return ips.find((ip) => ip.includes('.')) ?? null;
  } catch {
    return null;
  }
}
