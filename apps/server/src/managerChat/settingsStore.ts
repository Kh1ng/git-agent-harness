/**
 * Manager chat backend selection: a global default plus optional per-profile
 * overrides. This is a WebUI-only preference (which backend answers manager
 * chat), not part of GAH's own dispatch config -- deliberately kept out of
 * config.toml/the Rust side. Follows the same local-JSON-file pattern as
 * coordinatorIdentity.ts.
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { DEFAULT_BACKEND_ID } from './registry.js';
import type { HelperRoutePreference } from '@git-agent-harness/contracts';

export interface ManagerChatSettings {
  defaultBackend: string;
  profileOverrides: Record<string, string>;
  /** Last model the user picked per "profile:backendId" pair. The ACP
   * connection itself only remembers this in memory (ProfileConnection.
   * currentModelId) -- if that connection is ever recreated (crash, quota
   * error, server restart), a fresh session reverts to the backend's own
   * default. This survives that. */
  modelOverrides: Record<string, string>;
  /** Last ACP-advertised thought level picked per profile/backend. */
  reasoningEffortOverrides: Record<string, string>;
  helperRoutes: HelperRoutePreference[];
}

function settingsPath(): string {
  return process.env.GAH_MANAGER_CHAT_SETTINGS_PATH || resolve(process.cwd(), 'config/manager-chat-settings.json');
}

export function readSettings(): ManagerChatSettings {
  const path = settingsPath();
  if (existsSync(path)) {
    try {
      const data = JSON.parse(readFileSync(path, 'utf8'));
      return {
        defaultBackend: typeof data.defaultBackend === 'string' ? data.defaultBackend : DEFAULT_BACKEND_ID,
        profileOverrides: typeof data.profileOverrides === 'object' && data.profileOverrides ? data.profileOverrides : {},
        modelOverrides: typeof data.modelOverrides === 'object' && data.modelOverrides ? data.modelOverrides : {},
        reasoningEffortOverrides:
          typeof data.reasoningEffortOverrides === 'object' && data.reasoningEffortOverrides
            ? data.reasoningEffortOverrides
            : {},
        helperRoutes: Array.isArray(data.helperRoutes) ? data.helperRoutes.filter(validHelperRoute) : []
      };
    } catch {
      // Fall through to defaults on a corrupt file rather than crash.
    }
  }
  return {
    defaultBackend: DEFAULT_BACKEND_ID,
    profileOverrides: {},
    modelOverrides: {},
    reasoningEffortOverrides: {},
    helperRoutes: []
  };
}

function boundedId(value: unknown): value is string {
  return typeof value === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(value);
}

function boundedText(value: unknown, limit: number): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= limit && !/[\0-\x1f\x7f]/.test(value);
}

export function validHelperRoute(value: unknown): value is HelperRoutePreference {
  if (!value || typeof value !== 'object') return false;
  const route = value as Partial<HelperRoutePreference>;
  return boundedText(route.profile, 128) && boundedId(route.sourceBackend)
    && (route.sourceBackendInstance === null || boundedId(route.sourceBackendInstance))
    && typeof route.enabled === 'boolean' && boundedId(route.backend)
    && (route.backendInstance === null || boundedId(route.backendInstance))
    && (route.model === null || boundedText(route.model, 256));
}

export function helperRouteFor(profile: string, backend: string, backendInstance: string | null): HelperRoutePreference | undefined {
  return readSettings().helperRoutes.find(route => route.profile === profile && route.sourceBackend === backend
    && route.sourceBackendInstance === backendInstance);
}

export function writeSettings(settings: ManagerChatSettings): void {
  const path = settingsPath();
  const dir = dirname(path);
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
  writeFileSync(path, JSON.stringify(settings, null, 2));
}

export function backendForProfile(profile: string): string {
  const settings = readSettings();
  return settings.profileOverrides[profile] ?? settings.defaultBackend;
}

/** Select the backend for one project's interactive default conversation. */
export function setBackendForProfile(profile: string, backendId: string): void {
  const settings = readSettings();
  settings.profileOverrides[profile] = backendId;
  writeSettings(settings);
}

function modelOverrideKey(profile: string, backendId: string): string {
  return `${profile}:${backendId}`;
}

export function modelOverrideForProfile(profile: string, backendId: string): string | undefined {
  return readSettings().modelOverrides[modelOverrideKey(profile, backendId)];
}

export function setModelOverrideForProfile(profile: string, backendId: string, modelId: string): void {
  const settings = readSettings();
  settings.modelOverrides[modelOverrideKey(profile, backendId)] = modelId;
  writeSettings(settings);
}

export function reasoningEffortOverrideForProfile(profile: string, backendId: string): string | undefined {
  return readSettings().reasoningEffortOverrides[modelOverrideKey(profile, backendId)];
}

export function setReasoningEffortOverrideForProfile(profile: string, backendId: string, effortId: string): void {
  const settings = readSettings();
  settings.reasoningEffortOverrides[modelOverrideKey(profile, backendId)] = effortId;
  writeSettings(settings);
}
