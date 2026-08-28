/**
 * Chat sessions bound to worktrees (WP2 session service).
 *
 * A session is the durable, portable object: its event log (sessionLog.ts,
 * same directory) and its git branch survive backend swaps, server restarts,
 * and worktree reclamation. The worktree is a disposable materialization of
 * the branch -- created on demand, reclaimed by `gah prune` when idle and
 * clean (the `gah-chat-<repo_id>-` prefix makes Rust prune see it), and
 * rematerialized from the branch on resume. Archiving saves uncommitted work
 * as a patch before removing the worktree; the branch always survives.
 *
 * Naming follows the dispatch conventions exactly (see src/prune.rs and
 * src/dispatch/workflows): branch `gah/chat/<repo_id>-<session>`, worktree
 * dir `gah-chat-<repo_id>-<session>` under defaults.worktree_base. repo_id
 * comes from ProfileSummary (config truth), never derived from `repo`, or
 * prune's prefix match would silently miss these worktrees.
 */

import { execFile } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { dirname, join, resolve } from 'node:path';
import { promisify } from 'node:util';
import type { ChatSessionSummary, ProfileSummary } from '@git-agent-harness/contracts';

const execFileAsync = promisify(execFile);

export interface ChatSessionStoreOptions {
  /** Override the state directory (tests). Same base as sessionLog. */
  stateDir?: string;
}

const chatSessionStoreOptions: ChatSessionStoreOptions = {};

export function setChatSessionStoreOptions(opts: ChatSessionStoreOptions): void {
  chatSessionStoreOptions.stateDir = opts.stateDir;
}

/** Shared mutable store options (read per call, so runtime/test changes apply). */
export { chatSessionStoreOptions };

/** Shared state base with sessionLog.ts's chatLogPath resolution. */
function stateBase(opts?: ChatSessionStoreOptions): string {
  const base =
    (opts ?? chatSessionStoreOptions).stateDir
    ?? process.env.GAH_CHAT_STATE_DIR
    ?? (process.env.XDG_STATE_HOME
      ? resolve(process.env.XDG_STATE_HOME, 'gah', 'chat')
      : (process.env.HOME
        ? resolve(process.env.HOME, '.local', 'state', 'gah', 'chat')
        : resolve(process.cwd(), 'config', 'chat')));
  return base;
}

function profileDir(profile: string, opts?: ChatSessionStoreOptions): string {
  return join(stateBase(opts), `project-${encodeURIComponent(profile)}`);
}

function indexPath(profile: string, opts?: ChatSessionStoreOptions): string {
  return join(profileDir(profile, opts), 'sessions.json');
}

interface SessionIndexFile {
  sessions: ChatSessionSummary[];
}

function readIndex(profile: string, opts?: ChatSessionStoreOptions): ChatSessionSummary[] {
  const path = indexPath(profile, opts);
  if (!existsSync(path)) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(path, 'utf8'));
  } catch (error) {
    throw new Error(
      `Invalid chat session index at ${path}: ${error instanceof Error ? error.message : String(error)}`
    );
  }
  if (typeof parsed !== 'object' || parsed === null || !Array.isArray((parsed as { sessions?: unknown }).sessions)) {
    throw new Error(`Invalid chat session index at ${path}: expected an object with a "sessions" array`);
  }
  return (parsed as SessionIndexFile).sessions.filter(
    (s) => typeof s?.id === 'string' && typeof s?.branch === 'string'
  );
}

function writeIndex(profile: string, sessions: ChatSessionSummary[], opts?: ChatSessionStoreOptions): void {
  const path = indexPath(profile, opts);
  mkdirSync(dirname(path), { recursive: true });
  const temporaryPath = `${path}.${process.pid}.tmp`;
  writeFileSync(temporaryPath, `${JSON.stringify({ sessions }, null, 2)}\n`);
  renameSync(temporaryPath, path);
}

function newSessionId(): string {
  return randomBytes(4).toString('hex');
}

/** Composite key for the per-conversation maps in ManagerChatManager and the
 * ACP adapters: distinct sessions of one profile get distinct queues and
 * backend connections. The bare profile key stays exactly 'default'. */
export function chatKey(profile: string, sessionId?: string): string {
  return sessionId && sessionId !== 'default' ? `${profile}#${sessionId}` : profile;
}

export function worktreeDirName(repoId: string, sessionId: string): string {
  return `gah-chat-${repoId}-${sessionId}`;
}

export function sessionBranchName(repoId: string, sessionId: string): string {
  return `gah/chat/${repoId}-${sessionId}`;
}

function git(repoPath: string, ...args: string[]): Promise<{ stdout: string; stderr: string }> {
  return execFileAsync('git', args, { cwd: repoPath, maxBuffer: 32 * 1024 * 1024 });
}

async function isDirty(worktreePath: string): Promise<boolean> {
  const { stdout } = await git(worktreePath, 'status', '--porcelain');
  return stdout.trim().length > 0;
}

export interface CreateSessionInput {
  profile: string;
  /** The profile's config facts (repo_id, local_path, worktree_base). */
  profileInfo: Pick<ProfileSummary, 'repo_id' | 'local_path' | 'worktree_base'>;
  backend: string;
  /** Model override for the backend; null = backend default. */
  model?: string | null;
  title?: string;
  /** Branch override (issue chats): e.g. `gah/issue/<repo>-42`. The
   * worktree keeps the gah-chat-<repo_id>-<session> dir name so prune
   * still sees it; only the branch differs. */
  branch?: string;
}

/**
 * Creates a session with a fresh worktree. When worktree_base is unset the
 * session still works -- turns run in the profile checkout (cwd =
 * local_path) -- it just isn't isolated; worktreePath stays null.
 */
export async function createSession(input: CreateSessionInput, opts?: ChatSessionStoreOptions): Promise<ChatSessionSummary> {
  const { profile, profileInfo, backend } = input;
  if (!existsSync(profileInfo.local_path)) {
    throw new Error(`Profile checkout not found at ${profileInfo.local_path}; cannot start a chat session`);
  }
  const sessionId = newSessionId();
  const now = Date.now();
  const record: ChatSessionSummary = {
    id: sessionId,
    profile,
    worktreePath: null,
    branch: input.branch ?? sessionBranchName(profileInfo.repo_id, sessionId),
    backend,
    model: input.model ?? null,
    title: input.title ?? null,
    createdAt: now,
    lastActiveAt: now,
    archivedAt: null
  };

  if (profileInfo.worktree_base.trim().length > 0) {
    const dir = join(profileInfo.worktree_base, worktreeDirName(profileInfo.repo_id, sessionId));
    mkdirSync(profileInfo.worktree_base, { recursive: true });
    await git(profileInfo.local_path, 'worktree', 'add', '-b', record.branch, dir);
    record.worktreePath = dir;
  }

  const sessions = readIndex(profile, opts);
  sessions.push(record);
  writeIndex(profile, sessions, opts);
  return record;
}

export function listSessions(profile: string, opts?: ChatSessionStoreOptions): ChatSessionSummary[] {
  return readIndex(profile, opts)
    .slice()
    .sort((a, b) => b.lastActiveAt - a.lastActiveAt);
}

export function getSession(profile: string, sessionId: string, opts?: ChatSessionStoreOptions): ChatSessionSummary | null {
  return readIndex(profile, opts).find((s) => s.id === sessionId) ?? null;
}

/** Changes a live session's backend and/or model and/or title. The worktree
 * is untouched: the next turn runs on the new backend/model in the same
 * directory -- the manual form of backend interchange. */
export function updateSession(
  profile: string,
  sessionId: string,
  patch: { backend?: string; model?: string | null; title?: string },
  opts?: ChatSessionStoreOptions
): ChatSessionSummary {
  const sessions = readIndex(profile, opts);
  const session = sessions.find((s) => s.id === sessionId);
  if (!session) throw new Error(`No chat session '${sessionId}' for profile '${profile}'`);
  if (session.archivedAt !== null) throw new Error(`Chat session '${sessionId}' is archived`);
  if (patch.backend !== undefined) session.backend = patch.backend;
  if (patch.model !== undefined) session.model = patch.model;
  if (patch.title !== undefined) session.title = patch.title;
  writeIndex(profile, sessions, opts);
  return session;
}

/** Effective turn cwd for a session: its worktree when it exists (re-
 * materialized from the branch if prune reclaimed it), else the profile
 * checkout. Returns null for unknown sessions. */
export async function resolveSessionCwd(
  profile: string,
  sessionId: string,
  profileInfo: Pick<ProfileSummary, 'repo_id' | 'local_path' | 'worktree_base'>,
  opts?: ChatSessionStoreOptions
): Promise<{ cwd: string; session: ChatSessionSummary } | null> {
  const session = getSession(profile, sessionId, opts);
  if (!session || session.archivedAt !== null) return null;
  if (!session.worktreePath) return { cwd: profileInfo.local_path, session };
  if (existsSync(session.worktreePath)) return { cwd: session.worktreePath, session };
  // Prune reclaimed the idle worktree; the branch survives -- rematerialize.
  await git(profileInfo.local_path, 'worktree', 'add', session.worktreePath, session.branch);
  return { cwd: session.worktreePath, session };
}

/** Records session activity (called after each turn). */
export function touchSession(profile: string, sessionId: string, opts?: ChatSessionStoreOptions): void {
  const sessions = readIndex(profile, opts);
  const session = sessions.find((s) => s.id === sessionId);
  if (!session || session.archivedAt !== null) return;
  session.lastActiveAt = Date.now();
  writeIndex(profile, sessions, opts);
}

/**
 * Archives a session. A dirty worktree is patched into the session's state
 * directory first (intent-to-add so untracked files are captured), then the
 * worktree is removed. The branch and event log always survive for resume.
 */
export async function archiveSession(
  profile: string,
  sessionId: string,
  profileInfo: Pick<ProfileSummary, 'local_path'>,
  opts?: ChatSessionStoreOptions
): Promise<ChatSessionSummary> {
  const sessions = readIndex(profile, opts);
  const session = sessions.find((s) => s.id === sessionId);
  if (!session) throw new Error(`No chat session '${sessionId}' for profile '${profile}'`);
  if (session.archivedAt !== null) return session;

  if (session.worktreePath && existsSync(session.worktreePath)) {
    if (await isDirty(session.worktreePath)) {
      // Stage everything so the patch includes untracked files; the worktree
      // is discarded immediately after, so mutating its index is harmless.
      await git(session.worktreePath, 'add', '-A');
      const { stdout } = await git(session.worktreePath, 'diff', '--cached');
      if (stdout.trim().length > 0) {
        const dir = join(profileDir(profile, opts), `session-${encodeURIComponent(sessionId)}`);
        mkdirSync(dir, { recursive: true });
        writeFileSync(join(dir, `archive-${Date.now()}.patch`), stdout);
      }
    }
    await git(profileInfo.local_path, 'worktree', 'remove', '--force', session.worktreePath);
  }

  session.archivedAt = Date.now();
  session.worktreePath = null;
  writeIndex(profile, sessions, opts);
  return session;
}
