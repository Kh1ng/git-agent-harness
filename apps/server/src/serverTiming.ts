import type { Response } from 'express';

/** Runs `work` and appends its duration to the response's `Server-Timing`
 * header, so a slow request shows where it waited in browser devtools. The
 * entry is recorded on failure too; a failed dependency is often the slow one. */
export async function timed<T>(res: Response, metric: string, work: () => Promise<T>): Promise<T> {
  const start = performance.now();
  try {
    return await work();
  } finally {
    if (!res.headersSent) res.append('Server-Timing', `${metric};dur=${(performance.now() - start).toFixed(1)}`);
  }
}
