#!/usr/bin/env node
import { createMockControlPlane, isMockScenarioName } from './controlPlane.js';

function option(name: string): string | undefined {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

const scenarioValue = option('--scenario') ?? process.env.GAH_MOCK_SCENARIO ?? 'normal';
if (!isMockScenarioName(scenarioValue)) {
  console.error(`Unknown mock scenario: ${scenarioValue}`);
  process.exit(1);
}

const port = Number(option('--port') ?? process.env.PORT ?? 3774);
const host = option('--host') ?? process.env.HOST ?? '127.0.0.1';
const controlPlane = createMockControlPlane({ scenario: scenarioValue });
const running = await controlPlane.listen(port, host);

console.log(`GAH mock control plane: ${running.baseUrl}`);
console.log(`Scenario: ${running.scenario()}`);
console.log(`Scenarios: ${running.baseUrl}/api/mock/scenarios`);

async function shutdown(): Promise<void> {
  await running.close();
  process.exit(0);
}

process.on('SIGINT', () => void shutdown());
process.on('SIGTERM', () => void shutdown());
