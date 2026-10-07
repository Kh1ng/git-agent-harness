import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

/** The web app this checkout builds, relative to `src/` and `dist/` alike. */
export const BUILT_WEB_ROOT = fileURLToPath(new URL('../../web/dist', import.meta.url));

/**
 * The directory the server serves the dashboard from, or null for none.
 * `GAH_WEB_ROOT` wins when set, and an empty value turns serving off (a host
 * whose own web server serves the dashboard). Unset, the server serves the
 * checkout's build when it exists, so a fresh install needs no separate web
 * server (#1327).
 */
export function resolveWebRoot(configured: string | undefined, builtRoot: string = BUILT_WEB_ROOT): string | null {
  if (configured !== undefined) return configured.trim() === '' ? null : resolve(configured);
  return existsSync(join(builtRoot, 'index.html')) ? builtRoot : null;
}
