/**
 * GAH CLI integration module
 * Replaces the broken rustBackend.ts stdin/stdout bridge with direct CLI subcommand execution
 */

import { spawn, ChildProcessWithoutNullStreams, SpawnOptions } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';
import { access } from 'node:fs/promises';
import { constants } from 'node:fs';

// Re-export for use in SessionManager
export type { ChildProcessWithoutNullStreams };

const __dirname = fileURLToPath(new URL('.', import.meta.url));

// Type definitions for GAH CLI JSON output
export interface StatusSnapshot {
  schema_version: number;
  generated_at: string;
  profile: ProfileIdentity;
  observations: Observations;
  merge_requests: any[];
  availability: ScopeStatusJson[];
  recent_ledger?: RecentLedgerSummary;
  constraints: Blocker[];
  blockers: Blocker[];
  errors: StatusError[];
  available_tickets: any[];
}

interface ProfileIdentity {
  profile: string;
  display_name: string;
  repo_id: string;
  provider: string;
  local_path: string;
  default_target_branch: string;
}

interface Observations {
  sync: ObservationStatus;
  availability: ObservationStatus;
  ledger: ObservationStatus;
}

interface ObservationStatus {
  status: string;
}

export interface ScopeStatusJson {
  backend: string;
  model?: string;
  quota_pool?: string;
  eligible_now: boolean;
  reason?: string;
  unavailable_until?: string;
  source?: string;
  last_error_summary?: string;
  observed_at?: string;
  scope?: string;
}

interface RecentLedgerSummary {
  most_recent_dispatch_timestamp: string;
  most_recent_effective_backend: string;
  most_recent_effective_model?: string;
  most_recent_work_id?: string;
  most_recent_mode: string;
  most_recent_validation_result?: string;
  most_recent_failure_class?: string;
  most_recent_failure_stage?: string;
  most_recent_branch?: string;
  most_recent_mr_url?: string;
  attempts_started?: number;
  attempts_completed?: number;
  human_required: boolean;
  routing_diagnostics?: any;
}

interface Blocker {
  kind: string;
  reason?: string;
  message?: string;
  backend?: string;
  model?: string;
  until?: string;
  source_reference?: string;
}

interface StatusError {
  kind: string;
  message: string;
  context?: Record<string, unknown>;
}

export interface ControllerEvent {
  timestamp: string;
  event_type: string;
  profile?: string;
  work_id?: string;
  action?: string;
  backend?: string;
  model?: string;
  [key: string]: unknown;
}

/**
 * Find the gah binary path using the same logic as rustBackend.ts
 */
async function findGahBinary(): Promise<string> {
  const possiblePaths = [
    resolve(__dirname, '../../../target/release/gah'),
    resolve(__dirname, '../../../target/debug/gah'),
    'gah'
  ];

  for (const path of possiblePaths) {
    try {
      await access(path, constants.X_OK);
      return path;
    } catch {
      // Try next path
    }
  }

  // Return 'gah' as fallback to try system PATH
  return 'gah';
}

/**
 * Execute gah status command and return parsed JSON output
 */
export async function runStatus(profile: string, configPath?: string): Promise<StatusSnapshot> {
  const gahPath = await findGahBinary();
  
  const args = ['status', '--profile', profile, '--json'];
  if (configPath) {
    args.push('--config', configPath);
  }

  const options: SpawnOptions = {
    stdio: ['ignore', 'pipe', 'pipe'],
    cwd: resolve(__dirname, '..'),
    env: {
      ...process.env,
      RUST_LOG: process.env.RUST_LOG || 'info'
    }
  };

  const gahProcess = spawn(gahPath, args, options);
  
  // Wait for process to complete and capture all output
  const [status] = await once(gahProcess, 'exit');
  
  const stdoutChunks: Buffer[] = [];
  const stderrChunks: Buffer[] = [];
  
  gahProcess.stdout.on('data', (chunk) => stdoutChunks.push(chunk));
  gahProcess.stderr.on('data', (chunk) => stderrChunks.push(chunk));
  
  const stdout = Buffer.concat(stdoutChunks).toString('utf8');
  const stderr = Buffer.concat(stderrChunks).toString('utf8');

  if (status !== 0) {
    throw new Error(`gah status failed with exit code ${status}: ${stderr || stdout}`);
  }

  try {
    return JSON.parse(stdout) as StatusSnapshot;
  } catch (parseError) {
    throw new Error(`Failed to parse gah status JSON output: ${parseError instanceof Error ? parseError.message : String(parseError)}. Output: ${stdout}`);
  }
}

/**
 * Execute gah dispatch command and stream output via callback
 * Returns a promise that resolves when the process exits
 */
export async function runDispatch(
  profile: string,
  mode: string,
  backend: string,
  target: string,
  onLine: (line: string) => void,
  configPath?: string,
  additionalArgs: string[] = []
): Promise<{ exitCode: number; stdout: string; stderr: string }> {
  const gahPath = await findGahBinary();
  
  const args = ['dispatch', '--profile', profile, '--mode', mode, '--backend', backend, '--target', target, ...additionalArgs];
  if (configPath) {
    args.push('--config', configPath);
  }

  const options: SpawnOptions = {
    stdio: ['ignore', 'pipe', 'pipe'],
    cwd: resolve(__dirname, '..'),
    env: {
      ...process.env,
      RUST_LOG: process.env.RUST_LOG || 'info'
    }
  };

  const gahProcess = spawn(gahPath, args, options);
  
  const stdoutChunks: Buffer[] = [];
  const stderrChunks: Buffer[] = [];
  
  gahProcess.stdout.on('data', (chunk) => {
    const text = chunk.toString('utf8');
    stdoutChunks.push(chunk);
    // Split by lines and emit each line
    const lines = text.split('\n');
    for (const line of lines) {
      if (line.trim()) {
        onLine(line);
      }
    }
  });
  
  gahProcess.stderr.on('data', (chunk) => {
    stderrChunks.push(chunk);
    const text = chunk.toString('utf8');
    const lines = text.split('\n');
    for (const line of lines) {
      if (line.trim()) {
        onLine(`[stderr] ${line}`);
      }
    }
  });

  const [exitCode] = await once(gahProcess, 'exit');
  
  const stdout = Buffer.concat(stdoutChunks).toString('utf8');
  const stderr = Buffer.concat(stderrChunks).toString('utf8');

  return { exitCode: exitCode || 0, stdout, stderr };
}

/**
 * Execute gah events command and return parsed JSON output
 */
export async function runEvents(profile: string, sinceIso: string, configPath?: string): Promise<ControllerEvent[]> {
  const gahPath = await findGahBinary();
  
  const args = ['events', '--profile', profile, '--since', sinceIso, '--json'];
  if (configPath) {
    args.push('--config', configPath);
  }

  const options: SpawnOptions = {
    stdio: ['ignore', 'pipe', 'pipe'],
    cwd: resolve(__dirname, '..'),
    env: {
      ...process.env,
      RUST_LOG: process.env.RUST_LOG || 'info'
    }
  };

  const gahProcess = spawn(gahPath, args, options);
  
  const [status] = await once(gahProcess, 'exit');
  
  const stdoutChunks: Buffer[] = [];
  const stderrChunks: Buffer[] = [];
  
  gahProcess.stdout.on('data', (chunk) => stdoutChunks.push(chunk));
  gahProcess.stderr.on('data', (chunk) => stderrChunks.push(chunk));
  
  const stdout = Buffer.concat(stdoutChunks).toString('utf8');
  const stderr = Buffer.concat(stderrChunks).toString('utf8');

  if (status !== 0) {
    throw new Error(`gah events failed with exit code ${status}: ${stderr || stdout}`);
  }

  try {
    const parsed = JSON.parse(stdout) as unknown;
    if (Array.isArray(parsed)) {
      return parsed as ControllerEvent[];
    }
    // Handle case where output is a single object or wrapped
    if (typeof parsed === 'object' && parsed !== null) {
      const obj = parsed as Record<string, unknown>;
      if (Array.isArray(obj.events)) {
        return obj.events as ControllerEvent[];
      }
      // Try to extract events array
      for (const key of Object.keys(obj)) {
        if (Array.isArray(obj[key as keyof typeof obj])) {
          return obj[key as keyof typeof obj] as unknown as ControllerEvent[];
        }
      }
    }
    throw new Error(`Unexpected gah events output format: ${stdout}`);
  } catch (parseError) {
    throw new Error(`Failed to parse gah events JSON output: ${parseError instanceof Error ? parseError.message : String(parseError)}. Output: ${stdout}`);
  }
}

/**
 * Get availability information from a status snapshot
 */
export function getAvailabilityFromStatus(snapshot: StatusSnapshot): Record<string, ScopeStatusJson> {
  const result: Record<string, ScopeStatusJson> = {};
  for (const scope of snapshot.availability) {
    // Use backend as key, or backend:model if model is present
    const key = scope.model ? `${scope.backend}:${scope.model}` : scope.backend;
    result[key] = scope;
  }
  return result;
}

/**
 * Check if a specific backend is available based on status snapshot
 */
export function isBackendAvailable(snapshot: StatusSnapshot, backend: string): boolean {
  for (const scope of snapshot.availability) {
    if (scope.backend === backend && scope.eligible_now) {
      return true;
    }
  }
  return false;
}

/**
 * Execute gah dispatch command and return the child process for control
 * This variant allows the caller to manage the process lifecycle (e.g., kill it)
 */
export function spawnDispatch(
  profile: string,
  mode: string,
  backend: string,
  target: string,
  onLine: (line: string) => void,
  configPath?: string,
  additionalArgs: string[] = []
): ChildProcessWithoutNullStreams {
  const gahPath = syncFindGahBinary();
  
  const args = ['dispatch', '--profile', profile, '--mode', mode, '--backend', backend, '--target', target, ...additionalArgs];
  if (configPath) {
    args.push('--config', configPath);
  }

  const options: SpawnOptions = {
    stdio: ['ignore', 'pipe', 'pipe'],
    cwd: resolve(__dirname, '..'),
    env: {
      ...process.env,
      RUST_LOG: process.env.RUST_LOG || 'info'
    }
  };

  const gahProcess = spawn(gahPath, args, options);
  
  gahProcess.stdout.on('data', (chunk) => {
    const text = chunk.toString('utf8');
    const lines = text.split('\n');
    for (const line of lines) {
      if (line.trim()) {
        onLine(line);
      }
    }
  });
  
  gahProcess.stderr.on('data', (chunk) => {
    const text = chunk.toString('utf8');
    const lines = text.split('\n');
    for (const line of lines) {
      if (line.trim()) {
        onLine(`[stderr] ${line}`);
      }
    }
  });

  return process;
}

/**
 * Synchronous version of findGahBinary for use in spawnDispatch
 */
function syncFindGahBinary(): string {
  const fs = require('node:fs');
  const possiblePaths = [
    resolve(__dirname, '../../../target/release/gah'),
    resolve(__dirname, '../../../target/debug/gah'),
    'gah'
  ];

  for (const path of possiblePaths) {
    try {
      fs.accessSync(path, fs.constants.X_OK);
      return path;
    } catch {
      // Try next path
    }
  }

  return 'gah';
}

export {
  findGahBinary,
  spawnDispatch
};

