/**
 * GAH CLI integration module
 * Provides TypeScript wrappers around the real `gah` CLI subcommands
 * Replaces the broken stdin/stdout bridge in rustBackend.ts
 */

import { spawn, spawnSync, SpawnOptions } from 'node:child_process';
import { userInfo } from 'node:os';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { accessSync, constants, mkdirSync, unlinkSync, writeFileSync } from 'node:fs';
import { AsyncTtlCache } from './asyncTtlCache.js';
import type {
  StatusSnapshot,
  QuotaSnapshot,
  ControllerEvent,
  ReportData,
  ReportSeriesData,
  ReportGroupBy,
  LedgerEntry,
  ConfigSummary,
  ConfigSetData,
  ConfigProfileSummary,
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
type ExecutableProbe = (path: string) => boolean;

function executableOnDisk(path: string): boolean {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

export function findGahBinary(isExecutable: ExecutableProbe = executableOnDisk): string {
  const possiblePaths = [
    resolve(__dirname, '../../../target/release/gah'),
    resolve(__dirname, '../../../target/debug/gah'),
    resolve(__dirname, '../../../target/release/git-agent-harness'),
    resolve(__dirname, '../../../target/debug/git-agent-harness'),
    'gah' // Try system PATH as fallback
  ];

  for (const path of possiblePaths) {
    if (isExecutable(path)) return path;
  }

  // Default to 'gah' which will use system PATH
  return 'gah';
}

const GAH_BINARY = findGahBinary();
const STATUS_CACHE_TTL_MS = 30_000;
const statusCache = new AsyncTtlCache<string, StatusSnapshot>(STATUS_CACHE_TTL_MS);

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
  const key = JSON.stringify([profile, config ?? null]);
  return statusCache.get(key, () => runStatusUncached(profile, config));
}

async function runStatusUncached(profile: string, config?: string): Promise<StatusSnapshot> {
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
 * Run `gah quota snapshot --profile <profile> --since <since> --json` and
 * parse the output. This is the canonical quota/usage/availability payload
 * consumed by Overview and Quota.
 */
export async function runQuota(
  options: { profile: string; since?: string; config?: string }
): Promise<QuotaSnapshot> {
  const args = ['quota', 'snapshot', '--profile', options.profile, '--json'];
  args.push('--since', options.since ?? '7d');
  if (options.config) {
    args.push('--config-path', options.config);
  }
  return runJsonCommand<QuotaSnapshot>(args, options.config);
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
 * Run `gah report --series --bucket <bucket> --since <since> --json` and
 * parse the output. Time-bucketed usage/cost/success-rate series for the
 * Telemetry trend chart (Issue #142). Additive: does not change the
 * aggregate `runReport` behavior.
 */
export async function runReportSeries(
  options: {
    since?: string;
    profile?: string;
    bucket?: string;
    config?: string;
  } = {}
): Promise<ReportSeriesData> {
  const args = ['report', '--json', '--series'];
  args.push('--since', options.since ?? '14d');
  args.push('--bucket', options.bucket ?? 'daily');
  if (options.profile) {
    args.push('--profile', options.profile);
  }
  if (options.config) {
    args.push('--config-path', options.config);
  }
  return runJsonCommand<ReportSeriesData>(args, options.config);
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
  max_parallel_workers?: number;
  /** Validation command timeout in seconds. */
  validation_timeout_seconds?: number;
  manager_wake_autonomy?: string;
  config?: string;
}

export function buildProfileAddArgs(options: ProfileAddOptions): string[] {
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
  if (options.max_parallel_workers !== undefined && options.max_parallel_workers !== null) {
    args.push('--max-parallel-workers', String(options.max_parallel_workers));
  }
  if (options.validation_timeout_seconds !== undefined && options.validation_timeout_seconds !== null) {
    args.push('--validation-timeout-seconds', String(options.validation_timeout_seconds));
  }
  if (options.manager_wake_autonomy) {
    args.push('--manager-wake-autonomy', options.manager_wake_autonomy);
  }
  
  if (options.config) {
    args.push('--config', options.config);
  }

  return args;
}

export async function runProfileAdd(options: ProfileAddOptions): Promise<void> {
  const args = buildProfileAddArgs(options);
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
  max_parallel_workers?: number | null;
  manager_wake_autonomy?: string | null;
  /** Validation command timeout in seconds. */
  validation_timeout_seconds?: number | null;
  clear?: string[];
  config?: string;
}

export function buildProfileSetArgs(options: ProfileSetOptions): string[] {
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
  if (options.max_parallel_workers !== undefined && options.max_parallel_workers !== null) {
    args.push('--max-parallel-workers', String(options.max_parallel_workers));
  } else if (options.clear?.includes('max_parallel_workers')) {
    args.push('--clear', 'max_parallel_workers');
  }
  if (options.validation_timeout_seconds !== undefined && options.validation_timeout_seconds !== null) {
    args.push('--validation-timeout-seconds', String(options.validation_timeout_seconds));
  } else if (options.clear?.includes('validation_timeout_seconds')) {
    args.push('--clear', 'validation_timeout_seconds');
  }
  if (options.manager_wake_autonomy !== undefined && options.manager_wake_autonomy !== null) {
    args.push('--manager-wake-autonomy', options.manager_wake_autonomy);
  } else if (options.clear?.includes('manager_wake_autonomy')) {
    args.push('--clear', 'manager_wake_autonomy');
  }
  appendClearArgs(
    args,
    options.clear,
    new Set([
      'max_parallel_workers',
      'manager_wake_autonomy',
      'validation_timeout_seconds',
    ])
  );
  
  if (options.config) {
    args.push('--config', options.config);
  }

  return args;
}

export async function runProfileSet(options: ProfileSetOptions): Promise<void> {
  const args = buildProfileSetArgs(options);
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

// ---------------------------------------------------------------------------
// `gah config set` / `gah config show --json` (Issue #194): global defaults
// such as `current_manager`, editable from the dashboard Settings UI. Like
// the profile CRUD above, these shell out to the real `gah` CLI so the
// canonical TOML config stays the single source of truth.
// ---------------------------------------------------------------------------

export interface ConfigSetOptions {
  current_manager?: string | null;
  clear?: string[];
  config?: string;
}

export function buildConfigSetArgs(options: ConfigSetOptions): string[] {
  const args = ['config', 'set'];

  if (options.current_manager !== undefined && options.current_manager !== null) {
    args.push('--current-manager', options.current_manager);
  } else if (options.clear?.includes('current_manager')) {
    args.push('--clear', 'current_manager');
  }

  appendClearArgs(args, options.clear, new Set(['current_manager']));

  if (options.config) {
    args.push('--config-path', options.config);
  }

  return args;
}

export async function runConfigSet(options: ConfigSetOptions): Promise<void> {
  const args = buildConfigSetArgs(options);
  return new Promise((resolve, reject) => {
    const child = spawn(GAH_BINARY, args, getSpawnOptions(options.config));

    let stderr = '';

    child.stderr?.on('data', (data) => {
      stderr += data.toString();
    });

    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`gah config set failed with exit code ${code}: ${stderr}`));
        return;
      }
      resolve(undefined);
    });

    child.on('error', (error) => {
      reject(new Error(`Failed to spawn gah: ${error instanceof Error ? error.message : String(error)}`));
    });
  });
}

export async function runConfigShow(config?: string): Promise<{ current_manager: string | null }> {
  const args = ['config', 'show', '--json'];
  if (config) {
    args.push('--config', config);
  }
  return runJsonCommand<{ current_manager: string | null }>(args, config);
}

interface ConfigShowResponse {
  current_manager: string | null;
  profile?: ConfigProfileSummary;
}

export async function runConfigShowProfile(
  profile: string,
  config?: string
): Promise<ConfigProfileSummary> {
  const args = ['config', 'show', '--json', '--profile', profile];
  if (config) {
    args.push('--config-path', config);
  }
  const response = await runJsonCommand<ConfigShowResponse>(args, config);
  if (!response.profile) {
    throw new Error(`gah config show returned no profile data for '${profile}'`);
  }
  return response.profile;
}

// ---------------------------------------------------------------------------
// `gah loop --profile <p>` start/stop/status from the dashboard. The
// dashboard deliberately controls a systemd *user* unit rather than spawning
// a detached loop itself. That gives every profile one lifecycle owner:
// systemd owns the loop and its whole cgroup, so Stop or a parent failure
// terminates every concurrent worker with it. Direct detached spawning here
// previously allowed a server restart to leave an unobservable orphan loop.
// ---------------------------------------------------------------------------

/** Same state-dir fallback chain as `loop_lock_path` in src/controller.rs,
 * so the PID file lives next to gah's own lock file.
 * When config is unavailable, retain previous fallback behavior for parity
 * with older environments where discovery fails. */
export function loopStateDir(
  configPath: string | null = getConfigPath() ?? null,
  env: NodeJS.ProcessEnv = process.env
): string {
  if (!configPath) {
    const base =
      env.XDG_STATE_HOME ||
      (env.HOME ? resolve(env.HOME, '.local/state') : '/tmp');
    return resolve(base, 'gah');
  }
  return resolve(dirname(configPath), '.gah-locks');
}

function appendClearArgs(
  args: string[],
  clearValues: string[] | undefined,
  excluded: Set<string>
): void {
  if (!clearValues?.length) return;

  const seen = new Set<string>();
  for (const key of clearValues) {
    if (excluded.has(key) || seen.has(key)) continue;
    args.push('--clear', key);
    seen.add(key);
  }
}

/** A durable acknowledgement that the operator intentionally stopped this
 * profile through the control plane. The watchdog reads the same marker so it
 * does not turn a dashboard Stop into an immediate, invisible restart. */
function loopManualStopFile(profile: string): string {
  return resolve(loopStateDir(), `loop-${profile.replace(/\//g, '_')}.manual-stop.json`);
}

function clearManualStop(profile: string): void {
  try {
    unlinkSync(loopManualStopFile(profile));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
  }
}

export interface LoopStatus {
  running: boolean;
  pid?: number;
  startedAt?: string;
  /** Which process manager owns this loop. `external` is a diagnostic-only
   * state; the dashboard never signals a process it does not own. */
  owner?: 'systemd' | 'external';
}

/** A diagnostic for loops that were not started by the systemd unit. It is
 * intentionally never used as a stop target: killing arbitrary PIDs recreates
 * the split ownership that this module is designed to prevent. */
export function findExternalLoopPid(profile: string): number | undefined {
  const escaped = profile.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const result = spawnSync('pgrep', ['-f', `gah loop --profile ${escaped}( |$)`], {
    encoding: 'utf8'
  });
  const first = result.stdout?.split('\n').map((l) => parseInt(l, 10)).find((n) => Number.isFinite(n));
  return first;
}

function loopServiceName(profile: string): string {
  // This is an instance name, not a shell argument. Keep it deliberately
  // narrow so an HTTP caller cannot address an unrelated systemd unit.
  if (!/^[A-Za-z0-9][A-Za-z0-9_.-]*$/.test(profile)) {
    throw new Error(`Invalid profile name for systemd loop unit: '${profile}'`);
  }
  return `gah-loop@${profile}.service`;
}

function systemdUserEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env };
  // A server installed as a system service under User=... does not inherit a
  // login shell's user-bus variables. Point systemctl at that user's lingering
  // systemd manager explicitly so dashboard controls work in both deployments.
  const runtimeDir = env.XDG_RUNTIME_DIR || `/run/user/${userInfo().uid}`;
  env.XDG_RUNTIME_DIR = runtimeDir;
  env.DBUS_SESSION_BUS_ADDRESS ||= `unix:path=${runtimeDir}/bus`;
  return env;
}

interface SystemdUnitStatus {
  activeState?: string;
  loadState?: string;
  mainPid?: number;
  startedAt?: string;
}

function readSystemdLoopStatus(profile: string): SystemdUnitStatus | undefined {
  const service = loopServiceName(profile);
  const result = spawnSync(
    'systemctl',
    ['--user', 'show', service, '--property=LoadState', '--property=ActiveState', '--property=MainPID', '--property=ActiveEnterTimestamp', '--no-pager'],
    { encoding: 'utf8', env: systemdUserEnv() }
  );
  if (result.error || result.status !== 0) return undefined;

  const values = new Map<string, string>();
  for (const line of (result.stdout ?? '').split('\n')) {
    const separator = line.indexOf('=');
    if (separator > 0) values.set(line.slice(0, separator), line.slice(separator + 1));
  }
  const mainPid = Number.parseInt(values.get('MainPID') ?? '', 10);
  return {
    loadState: values.get('LoadState'),
    activeState: values.get('ActiveState'),
    mainPid: Number.isFinite(mainPid) && mainPid > 0 ? mainPid : undefined,
    startedAt: values.get('ActiveEnterTimestamp') || undefined
  };
}

function runSystemctlUser(action: 'start' | 'stop', profile: string): { ok: boolean; error?: string } {
  const service = loopServiceName(profile);
  const result = spawnSync('systemctl', ['--user', action, service, '--no-pager'], {
    encoding: 'utf8',
    env: systemdUserEnv()
  });
  if (!result.error && result.status === 0) return { ok: true };
  const detail = [result.stderr, result.stdout].filter(Boolean).join('\n').trim();
  return {
    ok: false,
    error:
      detail ||
      result.error?.message ||
      `systemctl --user ${action} ${service} exited with status ${result.status ?? 'unknown'}`
  };
}

/** The systemd unit is authoritative. An externally-found loop is surfaced
 * for incident diagnosis, but cannot be mistaken for a dashboard-owned one. */
export function getLoopStatus(profile: string): LoopStatus {
  const systemd = readSystemdLoopStatus(profile);
  if (systemd && ['active', 'activating', 'deactivating'].includes(systemd.activeState ?? '')) {
    return {
      running: true,
      pid: systemd.mainPid,
      startedAt: systemd.startedAt,
      owner: 'systemd'
    };
  }
  const externalPid = findExternalLoopPid(profile);
  if (externalPid !== undefined) {
    return { running: true, pid: externalPid, owner: 'external' };
  }
  return { running: false, owner: 'systemd' };
}

export interface StartLoopResult {
  started: boolean;
  pid?: number;
  alreadyRunning?: boolean;
  error?: string;
}

export async function startLoop(profile: string): Promise<StartLoopResult> {
  const existing = getLoopStatus(profile);
  if (existing.running) {
    return {
      started: false,
      alreadyRunning: true,
      pid: existing.pid,
      error:
        existing.owner === 'external'
          ? `An unmanaged gah loop is already running for profile '${profile}'. Stop its owning service before starting ${loopServiceName(profile)}.`
          : undefined
    };
  }

  const result = runSystemctlUser('start', profile);
  if (!result.ok) return { started: false, error: result.error };

  const status = getLoopStatus(profile);
  if (!status.running || status.owner !== 'systemd') {
    return {
      started: false,
      error: `${loopServiceName(profile)} accepted the start request but is not active; inspect it with systemctl --user status ${loopServiceName(profile)}.`
    };
  }
  // Only a confirmed systemd start clears the marker. A failed request must
  // leave an intentional stop intentional rather than inviting watchdog churn.
  clearManualStop(profile);
  return { started: true, pid: status.pid };
}

export interface StopLoopResult {
  stopped: boolean;
  error?: string;
}

/** Graceful operator stop. Persist a marker consumed by the watchdog before
 * signalling the loop, so the control-plane Stop action cannot be undone by
 * an automatic watchdog restart. The next successful control-plane Start
 * clears it. */
export function stopLoop(profile: string): StopLoopResult {
  const status = getLoopStatus(profile);
  if (!status.running) {
    return { stopped: false, error: `No running loop found for profile '${profile}'` };
  }
  if (status.owner !== 'systemd') {
    return {
      stopped: false,
      error: `Refusing to signal unmanaged loop PID ${status.pid ?? 'unknown'}; stop its owning service, then use ${loopServiceName(profile)}.`
    };
  }
  try {
    const result = runSystemctlUser('stop', profile);
    if (!result.ok) return { stopped: false, error: result.error };
    // Only a confirmed stop persists the marker. Writing it before the
    // systemctl call succeeds would leave the watchdog believing a still-
    // running loop (e.g. a bus-connection error, or linger not enabled) was
    // intentionally stopped, suppressing any respawn/alerting for it.
    mkdirSync(loopStateDir(), { recursive: true });
    writeFileSync(loopManualStopFile(profile), JSON.stringify({ stoppedAt: new Date().toISOString() }));
    return { stopped: true };
  } catch (error) {
    return { stopped: false, error: error instanceof Error ? error.message : String(error) };
  }
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
