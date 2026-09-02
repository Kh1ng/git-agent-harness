import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

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
