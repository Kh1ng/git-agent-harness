/**
 * Manager-chat adapter for Hermes.
 *
 * MVP shape: one `hermes chat` CLI invocation per turn, using `--resume` for
 * conversation continuity. Deliberately not the desktop app's JSON-RPC/
 * WebSocket protocol (`hermes serve`) -- that's built for editor/desktop
 * integration and a real implementation of it is much larger than this MVP
 * needs. `--worktree` is intentionally omitted: that flag is for coding
 * tasks, a chat session has no reason to create/tear down a git worktree
 * per message.
 *
 * Other managers (Codex, Claude) get their own adapter module later,
 * following the same shape: spawn per turn, parse session id + reply text,
 * go through the same shared memoryGatewayClient. This file intentionally
 * does not introduce a formal adapter interface yet -- ManagerChatManager
 * only knows about Hermes today, and designing the interface before a
 * second real implementation exists is how #520 stalled in the first place.
 */

import { spawn } from 'child_process';

export interface HermesTurnResult {
  sessionId: string;
  reply: string;
}

// `hermes chat -Q` prints the reply on stdout and `session_id: <id>` on stderr.
const SESSION_ID_RE = /^session_id:\s*(\S+)\s*$/m;
// Status chrome Hermes mixes into stdout, not conversation content.
const NOISE_LINE_RE = /^(Warning: Unknown toolsets:|↻ Resumed session)/;

function parseHermesOutput(stdout: string, stderr: string): HermesTurnResult {
  const sessionMatch = SESSION_ID_RE.exec(stderr);
  if (!sessionMatch) {
    throw new Error(`hermes chat did not print a session_id; stderr:\n${stderr}\nstdout:\n${stdout}`);
  }
  const reply = stdout
    .split('\n')
    .filter((line) => !NOISE_LINE_RE.test(line))
    .join('\n')
    .trim();
  return { sessionId: sessionMatch[1], reply };
}

/** Runs one turn. `resumeSessionId` undefined starts a fresh conversation. */
export function runHermesTurn(
  message: string,
  resumeSessionId: string | undefined,
  timeoutMs = 120_000
): Promise<HermesTurnResult> {
  return new Promise((resolve, reject) => {
    const args = ['-p', 'gah-manager', 'chat', '-Q'];
    if (resumeSessionId) args.push('--resume', resumeSessionId);
    args.push('-q', message);

    const child = spawn('hermes', args, { stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    const timer = setTimeout(() => {
      child.kill('SIGTERM');
      reject(new Error(`hermes chat timed out after ${timeoutMs}ms`));
    }, timeoutMs);

    child.stdout.on('data', (chunk) => (stdout += chunk.toString()));
    child.stderr.on('data', (chunk) => (stderr += chunk.toString()));
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      if (code !== 0) {
        reject(new Error(`hermes chat exited ${code}: ${stderr || stdout}`));
        return;
      }
      try {
        resolve(parseHermesOutput(stdout, stderr));
      } catch (error) {
        reject(error);
      }
    });
  });
}
