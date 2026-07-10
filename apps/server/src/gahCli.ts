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

// Profile CRUD operations for Issue #148
export interface ProfileAddOptions {
  name: string;
  display_name: string;
  repo_id: string;
  provider: string;
  repo: string;
  local_path: string;
  artifact_root: string;
  default_target_branch?: string;
  provider_api_base?: string;
  provider_project_id?: string;
  openhands_args?: string[];
  codex_args?: string[];
  codex_path?: string;
  claude_args?: string[];
  claude_path?: string;
  agy_path?: string;
  vibe_args?: string[];
  vibe_path?: string;
  opencode_args?: string[];
  opencode_path?: string;
  agy_second_home?: string;
  notify_command?: string;
  policy_path?: string;
  env_file?: string;
  env_file_prod?: string;
  validation_commands?: string[];
  auto_fix_commands?: string[];
  config?: string;
}

export async function runProfileAdd(options: ProfileAddOptions): Promise<void> {
  const args = ['profile', 'add', options.name];
  
  args.push('--display-name', options.display_name);
  args.push('--repo-id', options.repo_id);
  args.push('--provider', options.provider);
  args.push('--repo', options.repo);
  args.push('--local-path', options.local_path);
  args.push('--artifact-root', options.artifact_root);
  
  if (options.default_target_branch) {
    args.push('--default-target-branch', options.default_target_branch);
  }
  if (options.provider_api_base) {
    args.push('--provider-api-base', options.provider_api_base);
  }
  if (options.provider_project_id) {
    args.push('--provider-project-id', options.provider_project_id);
  }
  if (options.openhands_args?.length) {
    args.push('--openhands-args', options.openhands_args.join(','));
  }
  if (options.codex_args?.length) {
    args.push('--codex-args', options.codex_args.join(','));
  }
  if (options.codex_path) {
    args.push('--codex-path', options.codex_path);
  }
  if (options.claude_args?.length) {
    args.push('--claude-args', options.claude_args.join(','));
  }
  if (options.claude_path) {
    args.push('--claude-path', options.claude_path);
  }
  if (options.agy_path) {
    args.push('--agy-path', options.agy_path);
  }
  if (options.vibe_args?.length) {
    args.push('--vibe-args', options.vibe_args.join(','));
  }
  if (options.vibe_path) {
    args.push('--vibe-path', options.vibe_path);
  }
  if (options.opencode_args?.length) {
    args.push('--opencode-args', options.opencode_args.join(','));
  }
  if (options.opencode_path) {
    args.push('--opencode-path', options.opencode_path);
  }
  if (options.agy_second_home) {
    args.push('--agy-second-home', options.agy_second_home);
  }
  if (options.notify_command) {
    args.push('--notify-command', options.notify_command);
  }
  if (options.policy_path) {
    args.push('--policy-path', options.policy_path);
  }
  if (options.env_file) {
    args.push('--env-file', options.env_file);
  }
  if (options.env_file_prod) {
    args.push('--env-file-prod', options.env_file_prod);
  }
  if (options.validation_commands?.length) {
    args.push('--validation-commands', options.validation_commands.join(','));
  }
  if (options.auto_fix_commands?.length) {
    args.push('--auto-fix-commands', options.auto_fix_commands.join(','));
  }
  
  if (options.config) {
    args.push('--config-path', options.config);
  }

  return new Promise((resolve, reject) => {
    const child = spawn(GAH_BINARY, args, getSpawnOptions(options.config));
    
    let stderr = '';
    
    child.stderr?.on('data', (data) => {
      stderr += data.toString();
    });
    
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`gah profile add failed with exit code ${code}: ${stderr}`));
        return;
      }
      resolve(undefined);
    });
    
    child.on('error', (error) => {
      reject(new Error(`Failed to spawn gah: ${error instanceof Error ? error.message : String(error)}`));
    });
  });
}

export interface ProfileSetOptions {
  name: string;
  display_name?: string;
  repo_id?: string;
  provider?: string;
  repo?: string;
  local_path?: string;
  artifact_root?: string;
  default_target_branch?: string;
  provider_api_base?: string | null;
  provider_project_id?: string | null;
  openhands_args?: string[];
  codex_args?: string[];
  codex_path?: string | null;
  claude_args?: string[];
  claude_path?: string | null;
  agy_path?: string | null;
  vibe_args?: string[];
  vibe_path?: string | null;
  opencode_args?: string[];
  opencode_path?: string | null;
  agy_second_home?: string | null;
  notify_command?: string | null;
  policy_path?: string | null;
  env_file?: string | null;
  env_file_prod?: string | null;
  validation_commands?: string[];
  auto_fix_commands?: string[];
  clear?: string[];
  config?: string;
}

export async function runProfileSet(options: ProfileSetOptions): Promise<void> {
  const args = ['profile', 'set', options.name];
  
  if (options.display_name) {
    args.push('--display-name', options.display_name);
  }
  if (options.repo_id) {
    args.push('--repo-id', options.repo_id);
  }
  if (options.provider) {
    args.push('--provider', options.provider);
  }
  if (options.repo) {
    args.push('--repo', options.repo);
  }
  if (options.local_path) {
    args.push('--local-path', options.local_path);
  }
  if (options.artifact_root) {
    args.push('--artifact-root', options.artifact_root);
  }
  if (options.default_target_branch) {
    args.push('--default-target-branch', options.default_target_branch);
  }
  if (options.provider_api_base !== undefined && options.provider_api_base !== null) {
    args.push('--provider-api-base', options.provider_api_base);
  }
  if (options.provider_project_id !== undefined && options.provider_project_id !== null) {
    args.push('--provider-project-id', options.provider_project_id);
  }
  if (options.openhands_args?.length) {
    args.push('--openhands-args', options.openhands_args.join(','));
  }
  if (options.codex_args?.length) {
    args.push('--codex-args', options.codex_args.join(','));
  }
  if (options.codex_path !== undefined && options.codex_path !== null) {
    args.push('--codex-path', options.codex_path);
  }
  if (options.claude_args?.length) {
    args.push('--claude-args', options.claude_args.join(','));
  }
  if (options.claude_path !== undefined && options.claude_path !== null) {
    args.push('--claude-path', options.claude_path);
  }
  if (options.agy_path !== undefined && options.agy_path !== null) {
    args.push('--agy-path', options.agy_path);
  }
  if (options.vibe_args?.length) {
    args.push('--vibe-args', options.vibe_args.join(','));
  }
  if (options.vibe_path !== undefined && options.vibe_path !== null) {
    args.push('--vibe-path', options.vibe_path);
  }
  if (options.opencode_args?.length) {
    args.push('--opencode-args', options.opencode_args.join(','));
  }
  if (options.opencode_path !== undefined && options.opencode_path !== null) {
    args.push('--opencode-path', options.opencode_path);
  }
  if (options.agy_second_home !== undefined && options.agy_second_home !== null) {
    args.push('--agy-second-home', options.agy_second_home);
  }
  if (options.notify_command !== undefined && options.notify_command !== null) {
    args.push('--notify-command', options.notify_command);
  }
  if (options.policy_path !== undefined && options.policy_path !== null) {
    args.push('--policy-path', options.policy_path);
  }
  if (options.env_file !== undefined && options.env_file !== null) {
    args.push('--env-file', options.env_file);
  }
  if (options.env_file_prod !== undefined && options.env_file_prod !== null) {
    args.push('--env-file-prod', options.env_file_prod);
  }
  if (options.validation_commands?.length) {
    args.push('--validation-commands', options.validation_commands.join(','));
  }
  if (options.auto_fix_commands?.length) {
    args.push('--auto-fix-commands', options.auto_fix_commands.join(','));
  }
  if (options.clear?.length) {
    args.push('--clear', options.clear.join(','));
  }
  
  if (options.config) {
    args.push('--config-path', options.config);
  }

  return new Promise((resolve, reject) => {
    const child = spawn(GAH_BINARY, args, getSpawnOptions(options.config));
    
    let stderr = '';
    
    child.stderr?.on('data', (data) => {
      stderr += data.toString();
    });
    
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`gah profile set failed with exit code ${code}: ${stderr}`));
        return;
      }
      resolve(undefined);
    });
    
    child.on('error', (error) => {
      reject(new Error(`Failed to spawn gah: ${error instanceof Error ? error.message : String(error)}`));
    });
  });
}

export interface ProfileRemoveOptions {
  name: string;
  force?: boolean;
  config?: string;
}

export async function runProfileRemove(options: ProfileRemoveOptions): Promise<void> {
  const args = ['profile', 'remove', options.name];
  
  if (options.force) {
    args.push('--force');
  }
  
  if (options.config) {
    args.push('--config-path', options.config);
  }

  return new Promise((resolve, reject) => {
    const child = spawn(GAH_BINARY, args, getSpawnOptions(options.config));
    
    let stderr = '';
    
    child.stderr?.on('data', (data) => {
      stderr += data.toString();
    });
    
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`gah profile remove failed with exit code ${code}: ${stderr}`));
        return;
      }
      resolve(undefined);
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
