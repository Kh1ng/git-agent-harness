import { spawn } from 'node:child_process';
import { activityPath, notifiableActivity, type ActivityEvent } from '@git-agent-harness/contracts';
import { findGahBinary } from './gahCli.js';

export type ActivityDelivery = (event: ActivityEvent) => void | Promise<void>;

/** Controller events are read back from the Rust event log. Rust already
 * delivered their channel message when it recorded them (and a worker's log
 * never reaches this feed), so the channel only takes events Node originates. */
function fromControllerLog(event: ActivityEvent): boolean {
  return event.id.startsWith('controller:');
}

function activityLine(event: ActivityEvent, baseUrl: string): string {
  const url = new URL(activityPath(event), baseUrl).href;
  return `[gah] ${event.title}: ${event.message} ${url}`.replace(/\s+/g, ' ').trim();
}

function run(label: string, command: string, args: string[], stdin?: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: ['pipe', 'ignore', 'pipe'] });
    let stderr = '';
    child.stderr?.on('data', (chunk) => { stderr = (stderr + chunk).slice(-500); });
    child.on('error', reject);
    child.on('close', (code) => code === 0 ? resolve() : reject(new Error(`${label} exited ${code}: ${stderr.trim()}`)));
    child.stdin.end(stdin ?? '');
  });
}

/** Telegram/Discord through `gah notify-send`, reusing the Rust channel code.
 * The CLI no-ops when no channel is configured. */
export function channelDelivery(baseUrl: string, gahBinary: () => string = findGahBinary): ActivityDelivery {
  return async (event) => {
    if (!notifiableActivity(event) || fromControllerLog(event)) return;
    await run('gah notify-send', gahBinary(), [
      'notify-send',
      '--title', event.title,
      '--message', event.message,
      '--url', new URL(activityPath(event), baseUrl).href
    ]);
  };
}

/** One shell hook for every notifiable event, piped as a single line on stdin
 * (the same shape as the Rust per-profile `notify_command`). */
export function commandDelivery(baseUrl: string, env: NodeJS.ProcessEnv = process.env): ActivityDelivery | undefined {
  let command = env.GAH_NOTIFY_COMMAND;
  if (!command && env.GAH_NODE_LIVENESS_NOTIFY_COMMAND) {
    console.log('GAH_NODE_LIVENESS_NOTIFY_COMMAND is deprecated; rename it to GAH_NOTIFY_COMMAND. It now receives every notifiable event.');
    command = env.GAH_NODE_LIVENESS_NOTIFY_COMMAND;
  }
  if (!command) return undefined;
  const hook = command;
  return async (event) => {
    if (!notifiableActivity(event)) return;
    await run('GAH_NOTIFY_COMMAND', 'sh', ['-c', hook], `${activityLine(event, baseUrl)}\n`);
  };
}

/** Fan one recorded event out to every delivery method. One failing method
 * never blocks the others. */
export function deliverToAll(deliveries: (ActivityDelivery | undefined)[]): ActivityDelivery {
  const active = deliveries.filter((delivery): delivery is ActivityDelivery => !!delivery);
  return async (event) => {
    const results = await Promise.allSettled(active.map((delivery) => delivery(event)));
    for (const result of results) {
      if (result.status === 'rejected') {
        console.error(`[activity] delivery failed for ${event.id}: ${result.reason instanceof Error ? result.reason.message : String(result.reason)}`);
      }
    }
  };
}
