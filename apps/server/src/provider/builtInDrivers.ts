/**
 * Built-in drivers configuration
 * After TICKET-113, we only keep GitHub and GitLab drivers since they provide
 * git platform functionality that isn't covered by gah status --json.
 * All AI provider drivers (Codex, Claude, Cursor, OpenCode, Grok, OpenHands, AGY, Vibe)
 * have been removed and replaced with direct gah CLI integration via gahCli.ts.
 */

import { GitHubDriver, type GitHubDriverEnv } from './Drivers/GitHubDriver.js';
import { GitLabDriver, type GitLabDriverEnv } from './Drivers/GitLabDriver.js';
import type { AnyProviderDriver, ProviderDriverEnv } from './ProviderDriver.js';

/**
 * Union of infrastructure services required to construct any built-in driver.
 * This is the union of all driver environment requirements.
 */
export type BuiltInDriversEnv =
  | GitHubDriverEnv
  | GitLabDriverEnv;

/**
 * Ordered list of built-in drivers.
 * Order matters for tie-breaking in UI presentation.
 * The registry itself is keyed by driverKind, so iteration order has no functional effect.
 *
 * Note: Only GitHub and GitLab drivers remain. AI providers are now handled
 * through gahCli.ts which shells out to the real gah CLI.
 */
export const BUILT_IN_DRIVERS: ReadonlyArray<AnyProviderDriver<BuiltInDriversEnv>> = [
  GitHubDriver,
  GitLabDriver,
];

/**
 * Get a driver by its kind
 */
export function getDriverByKind(kind: string): AnyProviderDriver<BuiltInDriversEnv> | undefined {
  return BUILT_IN_DRIVERS.find(driver => driver.kind === kind);
}

/**
 * Get all supported provider kinds
 */
export function getSupportedDriverKinds(): string[] {
  return BUILT_IN_DRIVERS.map(driver => driver.kind);
}

/**
 * Check if a provider kind is supported
 */
export function isDriverSupported(kind: string): boolean {
  return BUILT_IN_DRIVERS.some(driver => driver.kind === kind);
}