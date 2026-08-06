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

export interface ManagerChatSettings {
  defaultBackend: string;
  profileOverrides: Record<string, string>;
  /** Last model the user picked per "profile:backendId" pair. The ACP
   * connection itself only remembers this in memory (ProfileConnection.
   * currentModelId) -- if that connection is ever recreated (crash, quota
   * error, server restart), a fresh session reverts to the backend's own
   * default. This survives that. */
  modelOverrides: Record<string, string>;
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
        modelOverrides: typeof data.modelOverrides === 'object' && data.modelOverrides ? data.modelOverrides : {}
      };
    } catch {
      // Fall through to defaults on a corrupt file rather than crash.
    }
  }
  return { defaultBackend: DEFAULT_BACKEND_ID, profileOverrides: {}, modelOverrides: {} };
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
