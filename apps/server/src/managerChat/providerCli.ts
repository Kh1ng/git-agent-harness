import { execFile } from 'node:child_process';
import { homedir } from 'node:os';
import { promisify } from 'node:util';
import type { ProfileSummary } from '@git-agent-harness/contracts';

const execFileAsync = promisify(execFile);

export const PROVIDER_CLI_LIMITS = {
  timeout: 10_000,
  maxBuffer: 5 * 1024 * 1024
} as const;

/** Runs one provider CLI command with the server-wide time and output limits. */
export async function execProviderCli(command: string, args: string[], cwd: string): Promise<{ stdout: string }> {
  const { stdout } = await execFileAsync(command, args, {
    cwd,
    encoding: 'utf8',
    ...PROVIDER_CLI_LIMITS
  });
  return { stdout };
}

/** Runs `gh`/`glab` for a project. A worker-only project has no checkout on
 * central (empty `local_path`), so the repository is named with `-R` instead
 * of inferred from the working directory (#1276). */
export type ForgeProject = Pick<ProfileSummary, 'provider' | 'repo' | 'web_url' | 'local_path'>;

export function execProjectCli(
  command: 'gh' | 'glab',
  args: string[],
  project: ForgeProject
): Promise<{ stdout: string }> {
  if (project.local_path) return execProviderCli(command, args, project.local_path);
  return execProviderCli(command, [...args, '-R', repositoryRef(project)], homedir());
}

export function repositoryRef(project: Pick<ProfileSummary, 'provider' | 'repo' | 'web_url'>): string {
  // glab accepts the full project URL, which also selects a self-hosted host.
  if (project.provider === 'gitlab') return project.web_url ?? project.repo;
  const host = project.web_url ? new URL(project.web_url).host : 'github.com';
  return host === 'github.com' ? project.repo : `${host}/${project.repo}`;
}
