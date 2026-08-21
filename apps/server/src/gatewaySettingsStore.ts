import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';

export interface GatewaySettings {
  url: string | null;
  apiKey: string | null;
  enabled: boolean;
  /** Per-profile opt-outs. If a profile name is in this set, the gateway is
   * skipped for that profile's recall/capture even if globally enabled. */
  disabledProfiles: string[];
}

function settingsPath(): string {
  return (
    process.env.GAH_GATEWAY_SETTINGS_PATH ||
    resolve(process.cwd(), 'config/gateway-settings.json')
  );
}

export function readGatewaySettings(): GatewaySettings {
  const path = settingsPath();
  if (existsSync(path)) {
    try {
      const data = JSON.parse(readFileSync(path, 'utf8'));
      return {
        url: typeof data.url === 'string' && data.url ? data.url : null,
        apiKey: typeof data.apiKey === 'string' && data.apiKey ? data.apiKey : null,
        enabled: typeof data.enabled === 'boolean' ? data.enabled : true,
        disabledProfiles: Array.isArray(data.disabledProfiles) ? data.disabledProfiles : [],
      };
    } catch {
      // fall through
    }
  }
  return { url: null, apiKey: null, enabled: true, disabledProfiles: [] };
}

export function writeGatewaySettings(settings: Partial<GatewaySettings>): void {
  const current = readGatewaySettings();
  const next: GatewaySettings = { ...current, ...settings };
  const path = settingsPath();
  const dir = dirname(path);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  writeFileSync(path, JSON.stringify(next, null, 2));
}

/** Effective gateway URL: store wins over env var. */
export function effectiveGatewayUrl(): string {
  const stored = readGatewaySettings().url;
  return stored || process.env.TDAI_GATEWAY_URL || 'http://127.0.0.1:8420';
}

/** Effective API key: store wins over env var. */
export function effectiveGatewayApiKey(): string | undefined {
  const stored = readGatewaySettings().apiKey;
  return stored || process.env.TDAI_GATEWAY_API_KEY || undefined;
}

/** Returns true if the gateway should be used for this profile. */
export function gatewayEnabledForProfile(profile?: string): boolean {
  const s = readGatewaySettings();
  if (!s.enabled) return false;
  if (profile && s.disabledProfiles.includes(profile)) return false;
  return true;
}
