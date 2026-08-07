#!/usr/bin/env node
/**
 * CLI entrypoint for registerNode (issue #881). Usage:
 *   npm run register-node --workspace=apps/server -- \
 *     --central-url https://central.example.com \
 *     --transport-mode authenticated_remote \
 *     --secret-ref env:NODE_TOKEN \
 *     [--self-url http://127.0.0.1:3773] [--labels a,b]
 *
 * COORDINATOR_TOKEN in the environment, if set, is sent as the central
 * node's Bearer auth (same var authMiddleware.ts checks).
 */

import type { RegisteredNode } from '@git-agent-harness/contracts';
import { registerNode } from './registerNode.js';

function parseArgs(argv: string[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) continue;
    const key = arg.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) {
      out[key] = 'true';
    } else {
      out[key] = next;
      i++;
    }
  }
  return out;
}

const VALID_TRANSPORT_MODES: RegisteredNode['transport_mode'][] = ['loopback', 'authenticated_remote', 'trusted_lan'];

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const centralUrl = args['central-url'];
  const transportMode = args['transport-mode'];
  const secretRef = args['secret-ref'];

  if (!centralUrl || !transportMode || !secretRef) {
    console.error(
      'Usage: register-node --central-url <url> --transport-mode <loopback|authenticated_remote|trusted_lan> --secret-ref <env:VAR|file:path> [--self-url <url>] [--labels a,b]'
    );
    process.exitCode = 1;
    return;
  }
  if (!VALID_TRANSPORT_MODES.includes(transportMode as RegisteredNode['transport_mode'])) {
    console.error(`Invalid --transport-mode '${transportMode}': must be one of ${VALID_TRANSPORT_MODES.join(', ')}`);
    process.exitCode = 1;
    return;
  }

  try {
    const result = await registerNode({
      centralUrl,
      selfUrl: args['self-url'],
      transportMode: transportMode as RegisteredNode['transport_mode'],
      secretRef,
      labels: args['labels']
        ? args['labels']
            .split(',')
            .map((l) => l.trim())
            .filter(Boolean)
        : undefined,
      token: process.env.COORDINATOR_TOKEN
    });
    console.log(`Registered ${result.node_id} (${result.display_name}) against ${centralUrl}`);
    if (result.warnings.length > 0) {
      console.warn(`Warnings: ${result.warnings.join('; ')}`);
    }
  } catch (err) {
    console.error(err instanceof Error ? err.message : String(err));
    process.exitCode = 1;
  }
}

main();
