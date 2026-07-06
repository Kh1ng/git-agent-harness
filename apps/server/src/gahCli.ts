/**
 * GAH CLI integration module
 * Provides TypeScript wrappers around the real `gah` CLI subcommands
 * Replaces the broken stdin/stdout bridge in rustBackend.ts
 */

import { spawn, SpawnOptions } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { accessSync, constants } from 'node:fs';

const __dirname = fileURLToPath(new URL('.', import.meta.url));

/**
 * Interface for the status snapshot returned by `gah status --json`
 * This is a subset of the full JSON output that we need for the server
 */
export interface StatusSnapshot {
  schema_version: number;
  generated_at: string;
  profile: {
    profile: string;
    display_name: string;
    repo_id: string;
    provider: string;
    local_path: string;
    default_target_branch: string;
  };
  observations: {
    sync: { status: string };
    availability: { status: string };
    ledger: { status: string };
  };
  availability: Array<{
    backend: string;
    model?: string;
    eligible_now: boolean;
    reason?: string;
    unavailable_until?: string;
    source?: string;
    last_error_summary?: string;
    observed_at: string;
    scope: string;
  }>;
  merge_requests: Array<{
    branch: string;
    work_id?: string;
    id: string;
    url: string;
    state: string;
    draft: boolean;
    merge_status: string;
    merged: boolean;
    classification: string;
    recommended_action: string;
  }>;
  recent_ledger: any[];
  constraints: any[];
  blockers: any[];
  errors: any[];
  available_tickets: any[];
}

/**
 * Interface for controller events returned by `gah events --json`
 */
export interface ControllerEvent {
  timestamp: string;
  event_type: string;
  profile?: string;
  details?: Record<string, unknown>;
}

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
