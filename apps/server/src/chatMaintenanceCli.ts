#!/usr/bin/env node

import type { ChatReclaimResult } from '@git-agent-harness/contracts';

function maintenanceUrl(): string {
  const configuredHost = process.env.HOST?.trim();
  const host = !configuredHost || configuredHost === '0.0.0.0'
    ? '127.0.0.1'
    : configuredHost === '::'
      ? '[::1]'
      : configuredHost.includes(':') && !configuredHost.startsWith('[')
        ? `[${configuredHost}]`
        : configuredHost;
  return `http://${host}:${process.env.PORT ?? '3773'}/api/manager-chat/reclaim`;
}

async function main(): Promise<void> {
  // The daily unit asks the already-running central server to sweep. Keeping
  // execution in that process lets maintenance see its in-flight chat set;
  // a standalone process could mistake a long turn for an idle session.
  const response = await fetch(maintenanceUrl(), {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ dryRun: false })
  });
  const body = await response.text();
  if (!response.ok) throw new Error(`${response.status} ${response.statusText}: ${body}`);
  const result = JSON.parse(body) as ChatReclaimResult;
  for (const warning of result.warnings) console.warn(warning);
  const reclaimed = result.candidates.reduce((sum, candidate) => sum + candidate.reclaimBytes, 0);
  console.log(`Chat maintenance: ${result.candidates.length} session(s), ${reclaimed} projected byte(s) reclaimed`);
}

main().catch((error) => {
  console.error(`Chat maintenance failed: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
