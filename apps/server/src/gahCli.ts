/**
 * GAH CLI integration module
 * Provides TypeScript wrappers around the real `gah` CLI subcommands
 * Replaces the broken stdin/stdout bridge in rustBackend.ts
 */

import { spawn, SpawnOptions } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { accessSync, constants } from 'node:fs';
import type {
  StatusSnapshot,
  ControllerEvent,
  ReportData,
  ReportGroupBy,
  LedgerEntry,
  ProfileSummary,
} from '@git-agent-harness/contracts';

const __dirname = fileURLToPath(new URL('.', import.meta.url));

// StatusSnapshot / ControllerEvent are now the real, field-accurate types
// from @git-agent-harness/contracts (packages/contracts/src/gah.ts) --
// mirrored 1:1 from src/status.rs and src/events.rs -- instead of a
// locally hand-rolled (and previously inaccurate: recent_ledger is a
// single nullable object, not an array) subset.
export type { StatusSnapshot, ControllerEvent };

/**
 * Find the GAH binary path
 * Reuses the same lookup logic from rustBackend.ts
 */
function findGahBinary(): string {
  const possiblePaths = [
    resolve(__dirname, '../../../target/release/gah'),
    resolve(__dirname, '../../../target/debug/gah'),
    resolve(__dirname, '../../../target/release/git-agent-harness'),
    resolve(__dirname, '../../../target/debug/git-agent-harness'),
    'gah' // Try system PATH as fallback
  ];

  for (const path of possiblePaths) {
    try {
      // Check if the path exists and is executable
      // Note: We use a simple sync check here for startup
      // In a real implementation, this could be async
      accessSync(path, constants.X_OK);
      return path;
    } catch {
      // Try next path
    }
  }

  // Default to 'gah' which will use system PATH
  return 'gah';
}

const GAH_BINARY = findGahBinary();

/**
 * Get the path to the GAH config file
 */
function getConfigPath(config?: string): string | undefined {
  if (config) {
    return config;
  }
  // Try to find config in standard locations
  const possiblePaths = [
    resolve(__dirname, '../../../gah-config.toml'),
    resolve(__dirname, '../../../config/gah-config.toml'),
    process.env.GAH_CONFIG_PATH,
    process.env.GAH_CANONICAL_CONFIG
  ];

  for (const path of possiblePaths) {
    if (path) {
      try {
        accessSync(path, constants.R_OK);
        return path;
      } catch {
        // Try next path
      }
    }
  }

  return undefined;
}

/**
 * Spawn options for running GAH CLI commands
 */
function getSpawnOptions(config?: string): SpawnOptions {
  const env = { ...process.env };
  
  // Set config path if provided
  if (config) {
    env.GAH_CONFIG_PATH = config;
  }
  
  return {
    cwd: resolve(__dirname, '..'),
    env,
    stdio: ['ignore', 'pipe', 'pipe']
  };
}

/**
 * Run `gah status --profile <profile> --json` and parse the output
 */
export async function runStatus(profile: string, config?: string): Promise<StatusSnapshot> {
  const args = ['status', '--profile', profile, '--json'];
  
  if (config) {
    args.push('--config-path', config);
  }

  return new Promise((resolve, reject) => {
    const child = spawn(GAH_BINARY, args, getSpawnOptions(config));
    
    let stdout = '';
    let stderr = '';
    
    child.stdout?.on('data', (data) => {
      stdout += data.toString();
    });
    
    child.stderr?.on('data', (data) => {
      stderr += data.toString();
    });
    
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`gah status failed with exit code ${code}: ${stderr || stdout}`));
        return;
      }
      
      try {
        const result = JSON.parse(stdout) as StatusSnapshot;
        resolve(result);
      } catch (parseError) {
        reject(new Error(`Failed to parse gah status output: ${parseError instanceof Error ? parseError.message : String(parseError)}
Output: ${stdout}`));
      }
    });
    
    child.on('error', (error) => {
      reject(new Error(`Failed to spawn gah: ${error instanceof Error ? error.message : String(error)}`));
    });
  });
}

/**
 * Shared plumbing for the read-only `--json` subcommands added alongside
 * runReport/runLedgerWork below -- same spawn/parse/error shape as
 * runStatus above, factored out so those two don't duplicate it.
 */
function runJsonCommand<T>(args: string[], config?: string): Promise<T> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(GAH_BINARY, args, getSpawnOptions(config));

    let stdout = '';
    let stderr = '';

    child.stdout?.on('data', (data) => {
      stdout += data.toString();
    });

    child.stderr?.on('data', (data) => {
      stderr += data.toString();
    });

    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`gah ${args[0]} failed with exit code ${code}: ${stderr || stdout}`));
        return;
      }
      try {
        resolvePromise(JSON.parse(stdout) as T);
      } catch (parseError) {
        reject(new Error(`Failed to parse gah ${args[0]} output: ${parseError instanceof Error ? parseError.message : String(parseError)}
Output: ${stdout}`));
      }
    });

    child.on('error', (error) => {
      reject(new Error(`Failed to spawn gah: ${error instanceof Error ? error.message : String(error)}`));
    });
  });
}

/**
 * Run `gah report --since <since> --group-by <groupBy> --json` and parse
 * the output. Backend/model performance comparison (tokens, cost,
 * success rate, quota observations) -- the data source for the Telemetry
 * page. Previously implemented in the Rust CLI (TICKET-098) but never
 * wired to the server.
 */
export async function runReport(
  options: { since?: string; profile?: string; groupBy?: ReportGroupBy; config?: string } = {}
): Promise<ReportData> {
  const args = ['report', '--json'];
  args.push('--since', options.since ?? '7d');
  args.push('--group-by', options.groupBy ?? 'backend');
  if (options.profile) {
    args.push('--profile', options.profile);
  }
  if (options.config) {
    args.push('--config-path', options.config);
  }
  return runJsonCommand<ReportData>(args, options.config);
}

/**
 * Run `gah ledger work <workId> --json` and parse the output: full ledger
 * history for one work item, chronological. The data source for the Work
 * detail/attempt-timeline view.
 */
export async function runLedgerWork(workId: string, config?: string): Promise<LedgerEntry[]> {
  const args = ['ledger', 'work', workId, '--json'];
  if (config) {
    args.push('--config-path', config);
  }
  return runJsonCommand<LedgerEntry[]>(args, config);
}

/**
 * Run `gah dispatch` with the given options
 * Streams stdout to the onLine callback as lines arrive
 * Returns when the process exits
 */
export interface DispatchOptions {
  profile: string;
  mode: string;
  backend?: string;
  target?: string;
  branch?: string;
  mr?: string;
  model?: string;
  budget?: number;
  dryRun?: boolean;
  configPath?: string;
  currentBranch?: boolean;
  retries?: number;
  allowDraftFail?: boolean;
  prod?: boolean;
  allowUnknownRedBaseline?: boolean;
  escalate?: boolean;
}

export interface DispatchResult {
  exitCode: number;
  stderr: string;
}

export async function runDispatch(
  options: DispatchOptions,
  onLine: (line: string) => void
): Promise<DispatchResult> {
  const args = ['dispatch', '--profile', options.profile, '--mode', options.mode];
  
  if (options.backend) {
    args.push('--backend', options.backend);
  }
  
  if (options.target) {
    args.push('--target', options.target);
  }
  
  if (options.branch) {
    args.push('--branch', options.branch);
  }
  
  if (options.mr) {
    args.push('--mr', options.mr);
  }
  
  if (options.model) {
    args.push('--model', options.model);
  }
  
  if (options.budget !== undefined) {
    args.push('--budget', options.budget.toString());
  }
  
  if (options.dryRun) {
    args.push('--dry-run');
  }
  
  if (options.currentBranch) {
    args.push('--current-branch');
  }
  
  if (options.retries !== undefined) {
    args.push('--retries', options.retries.toString());
  }
  
  if (options.allowDraftFail) {
    args.push('--allow-draft-fail');
  }
  
  if (options.prod) {
    args.push('--prod');
  }
  
  if (options.allowUnknownRedBaseline) {
    args.push('--allow-unknown-red-baseline');
  }
  
  if (options.escalate) {
    args.push('--escalate');
  }
  
  if (options.configPath) {
    args.push('--config-path', options.configPath);
  }

  return new Promise((resolve) => {
    const child = spawn(GAH_BINARY, args, getSpawnOptions(options.configPath));
    
    let stderr = '';
    
    child.stdout?.on('data', (data) => {
      const text = data.toString();
      // Split by newlines and forward each line
      const lines = text.split('\n');
      for (const line of lines) {
        if (line.trim()) {
          onLine(line);
        }
      }
    });
    
    child.stderr?.on('data', (data) => {
      stderr += data.toString();
    });
    
    child.on('close', (code) => {
      resolve({
        exitCode: code || 0,
        stderr
      });
    });
    
    child.on('error', (error) => {
      // Still resolve with error info
      resolve({
        exitCode: -1,
        stderr: `Failed to spawn gah: ${error instanceof Error ? error.message : String(error)}`
      });
    });
  });
}

/**
 * Run `gah events --profile <profile> --since <since> --json`
 * and parse the output
 */
export async function runEvents(profile: string, sinceIso: string, config?: string): Promise<ControllerEvent[]> {
  const args = ['events', '--profile', profile, '--since', sinceIso, '--json'];
  
  if (config) {
    args.push('--config-path', config);
  }

  return new Promise((resolve, reject) => {
    const child = spawn(GAH_BINARY, args, getSpawnOptions(config));
    
    let stdout = '';
    let stderr = '';
    
    child.stdout?.on('data', (data) => {
      stdout += data.toString();
    });
    
    child.stderr?.on('data', (data) => {
      stderr += data.toString();
    });
    
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`gah events failed with exit code ${code}: ${stderr || stdout}`));
        return;
      }
      
      try {
        const result = JSON.parse(stdout) as ControllerEvent[];
        resolve(result);
      } catch (parseError) {
        reject(new Error(`Failed to parse gah events output: ${parseError instanceof Error ? parseError.message : String(parseError)}
Output: ${stdout}`));
      }
    });
    
    child.on('error', (error) => {
      reject(new Error(`Failed to spawn gah: ${error instanceof Error ? error.message : String(error)}`));
    });
  });
}

/**
 * Run `gah profile list --json`: the configured profiles (name, repo,
 * provider, web URL) straight from the TOML config -- re-read fresh on
 * every call, so adding/removing a `[profiles.x]` block shows up on the
 * next fetch with no server restart.
 */
export async function runProfileList(config?: string): Promise<ProfileSummary[]> {
  const args = ['profile', 'list', '--json'];
  if (config) {
    args.push('--config', config);
  }
  return runJsonCommand<ProfileSummary[]>(args, config);
}

/**
 * Get the path to the GAH binary (for debugging/testing)
 */
export function getGahBinaryPath(): string {
  return GAH_BINARY;
}

/**
 * Whether the resolved gah binary actually runs. `findGahBinary()` always
 * returns a path (falling back to the bare `'gah'` string on PATH even if
 * nothing resolves), so checking `GAH_BINARY` alone can't tell availability
 * -- this has to actually try spawning it.
 */
export function isGahCliAvailable(): Promise<boolean> {
  return new Promise((resolvePromise) => {
    const child = spawn(GAH_BINARY, ['--help'], { stdio: 'ignore' });
    child.on('error', () => resolvePromise(false));
    child.on('close', (code) => resolvePromise(code === 0));
  });
}
