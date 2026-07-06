/**
 * Built-in drivers configuration
 * Lists all the provider drivers that are available in this build
 */

import { GitHubDriver, type GitHubDriverEnv } from './Drivers/GitHubDriver.js';
import { GitLabDriver, type GitLabDriverEnv } from './Drivers/GitLabDriver.js';
import type { AnyProviderDriver, ProviderDriverEnv } from './ProviderDriver.js';

// Import stub drivers for t3code providers we don't fully implement yet
import { CodexDriver, type CodexDriverEnv } from './Drivers/CodexDriver.js';
import { ClaudeDriver, type ClaudeDriverEnv } from './Drivers/ClaudeDriver.js';
import { CursorDriver, type CursorDriverEnv } from './Drivers/CursorDriver.js';
import { OpenCodeDriver, type OpenCodeDriverEnv } from './Drivers/OpenCodeDriver.js';
import { GrokDriver, type GrokDriverEnv } from './Drivers/GrokDriver.js';
import { OpenHandsDriver, type OpenHandsDriverEnv } from './Drivers/OpenHandsDriver.js';
import { AGYDriver, type AGYDriverEnv } from './Drivers/AGYDriver.js';
import { VibeDriver, type VibeDriverEnv } from './Drivers/VibeDriver.js';

/**
 * Union of infrastructure services required to construct any built-in driver.
 * This is the union of all driver environment requirements.
 */
export type BuiltInDriversEnv =
  | GitHubDriverEnv
  | GitLabDriverEnv
  | CodexDriverEnv
  | ClaudeDriverEnv
  | CursorDriverEnv
  | OpenCodeDriverEnv
  | GrokDriverEnv
  | OpenHandsDriverEnv
  | AGYDriverEnv
  | VibeDriverEnv;

/**
 * Ordered list of built-in drivers.
 * Order matters for tie-breaking in UI presentation.
 * The registry itself is keyed by driverKind, so iteration order has no functional effect.
 */
export const BUILT_IN_DRIVERS: ReadonlyArray<AnyProviderDriver<BuiltInDriversEnv>> = [
  GitHubDriver,
  GitLabDriver,
  CodexDriver,
  ClaudeDriver,
  CursorDriver,
  OpenCodeDriver,
  GrokDriver,
  OpenHandsDriver,
  AGYDriver,
  VibeDriver,
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