import type { RequestHandler } from 'express';
import { createHash } from 'node:crypto';
import { closeSync, fstatSync, fsyncSync, mkdirSync, openSync, readFileSync, readSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

type Operation = 'loop.start' | 'loop.stop' | 'hold.set' | 'hold.clear' | 'availability.clear' | 'ledger.clear_attempts';
const digest = (value: string) => createHash('sha256').update(value).digest('hex');

// JSON object order is not part of the request's meaning; array order is.
function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, item) => item && typeof item === 'object' && !Array.isArray(item)
    ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
}

/** Protect fixed, non-streaming HTTP operations after authentication. A durable
 * exclusive receipt prevents repeats across restarts and concurrent servers.
 * Duplicate requests return 409: callers must inspect current state, never
 * silently retry with a new key. No response bodies or credentials are stored.
 */
export function mutationSafety(nodeId: string, directory = process.env.GAH_MUTATION_STORE_PATH ?? resolve('config/mutations')): (operation: Operation) => RequestHandler {
  function syncDirectory(): void {
    const folder = openSync(directory, 'r');
    try { fsyncSync(folder); } finally { closeSync(folder); }
  }

  function append(record: object): void {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const file = openSync(join(directory, 'audit.jsonl'), 'a+', 0o600);
    try {
      const size = fstatSync(file).size;
      const tail = Buffer.alloc(1);
      if (size > 0 && (readSync(file, tail, 0, 1, size - 1) !== 1 || tail[0] !== 10)) throw new Error('Incomplete audit record');
      writeFileSync(file, `${JSON.stringify({ schema_version: 1, timestamp: new Date().toISOString(), node_id: nodeId, ...record })}\n`);
      fsyncSync(file);
    }
    finally { closeSync(file); }
    // Persist the directory entry too when this append first creates the log.
    syncDirectory();
  }

  return operation => (req, res, next) => {
    res.setHeader('Cache-Control', 'no-store');
    const principal = res.locals.authPrincipal;
    const actor = principal?.kind === 'owner' ? 'owner' : principal?.kind === 'device' ? `device:${principal.id}` : 'unauthenticated';
    const key = req.get('Idempotency-Key');
    const target = digest(canonical({ body: req.body ?? null, query: req.query }));
    const operationId = digest(canonical([nodeId, actor, operation, key ?? null]));
    const record = { actor, operation, operation_id: operationId, target_digest: target };
    const reject = (status: number, code: string, message: string) => {
      try { append({ ...record, result: 'rejected', reason: code, status }); }
      catch { return res.status(503).json({ error: 'mutation_storage_unavailable', message: 'Cannot record this operation. No new action was started.' }); }
      return res.status(status).json({ error: code, message, operationId });
    };
    if (actor === 'unauthenticated') return reject(401, 'authentication_required', 'Authenticate before changing node state.');
    if (operation === 'ledger.clear_attempts' && principal.kind !== 'owner') return reject(403, 'owner_required', 'Use owner access to clear attempt history.');
    if (!key || !/^[A-Za-z0-9_-]{16,128}$/.test(key)) return reject(400, 'idempotency_key_required', 'Supply an Idempotency-Key of 16–128 letters, digits, underscores, or hyphens.');

    try {
      mkdirSync(directory, { recursive: true, mode: 0o700 });
      // ponytail: receipts are retained indefinitely; add indexed archival if
      // operator traffic makes this directory large, never expire replay guards.
      const path = join(directory, `${operationId}.json`);
      let file: number;
      try { file = openSync(path, 'wx', 0o600); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
        // Malformed/partial receipts fail closed. They never authorize a retry.
        const previous = JSON.parse(readFileSync(path, 'utf8'));
        if (previous.target_digest !== target) return reject(409, 'idempotency_conflict', 'This key was already used for different input.');
        return reject(409, 'mutation_already_accepted', 'This operation was already accepted and will not run again. Refresh its status before taking another action.');
      }
      try { writeFileSync(file, JSON.stringify(record)); fsyncSync(file); }
      finally { closeSync(file); }
      syncDirectory();
      append({ ...record, result: 'accepted', reason: 'authorized' });
    } catch {
      return res.status(503).json({ error: 'mutation_storage_unavailable', message: 'Cannot record this operation. No new action was started.', operationId });
    }

    // These six handlers end with JSON. Record the outcome before acknowledging
    // it; a failed terminal write retains the receipt and cannot rerun the action.
    const json = res.json.bind(res);
    res.json = body => {
      res.json = json;
      try { append({ ...record, result: res.statusCode < 400 ? 'completed' : 'failed', reason: 'http_result', status: res.statusCode }); }
      catch { return res.status(503).json({ error: 'mutation_outcome_unknown', message: 'The operation may have completed, but its outcome could not be recorded. Refresh status before taking another action.', operationId }); }
      return json(body);
    };
    res.setHeader('X-GAH-Operation-Id', operationId);
    next();
  };
}
