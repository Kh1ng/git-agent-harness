/**
 * Built-in drivers configuration
 * Lists all the provider drivers that are available in this build
 * 
 * NOTE: Drivers have been replaced with direct GAH CLI integration via gahCli.ts
 * as per TICKET-113. This file is kept for backward compatibility but exports an
 * empty list since all provider operations now go through the CLI.
 */

import type { AnyProviderDriver, ProviderDriverEnv } from './ProviderDriver.js';

/**
 * Union of infrastructure services required to construct any built-in driver.
 * Currently empty as all drivers have been replaced with CLI integration.
 */
export type BuiltInDriversEnv = ProviderDriverEnv;

/**
 * Ordered list of built-in drivers.
 * Empty as per TICKET-113 - all provider operations now use the GAH CLI directly.
 */
export const BUILT_IN_DRIVERS: ReadonlyArray<AnyProviderDriver<BuiltInDriversEnv>> = [];

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