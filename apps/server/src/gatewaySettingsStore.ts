import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';

/** Per-profile memory context policy (#961). Everything is optional so a
 * profile with no gateway configured is completely unaffected -- no new
 * prompt prefix, no new settings requirement, no new failure mode. */
export interface MemoryContextPolicy {
  /** Hard char budget for injected recall context per turn. Unset/0 = no
   * truncation (today's behavior). Over-budget recall is truncated to the
   * highest-relevance head, deterministically, and marked truncated. */
  budgetChars?: number;
  /** Memory tiers eligible for injection (L0 conversational / L1 extracted /
   * L2 consolidated). Recorded for provenance; the gateway's flat /recall
   * blob is what it is -- GAH never re-tiers. */
  tiers?: string[];
}

export interface GatewaySettings {
  url: string | null;
  apiKey: string | null;
  enabled: boolean;
  /** Per-profile opt-outs. If a profile name is in this set, the gateway is
   * skipped for that profile's recall/capture even if globally enabled. */
  disabledProfiles: string[];
  /** Global default context policy. */
  contextPolicy: MemoryContextPolicy;
  /** Per-profile overrides, merged over the global default. */
  contextPolicies: Record<string, MemoryContextPolicy>;
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
      const policy = typeof data.contextPolicy === 'object' && data.contextPolicy
        ? normalizePolicy(data.contextPolicy)
        : {};
      const perProfile: Record<string, MemoryContextPolicy> = {};
      if (data.contextPolicies && typeof data.contextPolicies === 'object') {
        for (const [name, value] of Object.entries(data.contextPolicies)) {
          if (value && typeof value === 'object') perProfile[name] = normalizePolicy(value as Record<string, unknown>);
        }
      }
      return {
        url: typeof data.url === 'string' && data.url ? data.url : null,
        apiKey: typeof data.apiKey === 'string' && data.apiKey ? data.apiKey : null,
        enabled: typeof data.enabled === 'boolean' ? data.enabled : true,
        disabledProfiles: Array.isArray(data.disabledProfiles) ? data.disabledProfiles : [],
        contextPolicy: policy,
        contextPolicies: perProfile,
      };
    } catch {
      // fall through
    }
  }
  return { url: null, apiKey: null, enabled: true, disabledProfiles: [], contextPolicy: {}, contextPolicies: {} };
}

function normalizePolicy(value: Record<string, unknown>): MemoryContextPolicy {
  const policy: MemoryContextPolicy = {};
  if (typeof value.budgetChars === 'number' && value.budgetChars > 0) policy.budgetChars = value.budgetChars;
  if (Array.isArray(value.tiers) && value.tiers.length > 0) {
    policy.tiers = value.tiers.filter((t): t is string => typeof t === 'string');
  }
  return policy;
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

/** Returns true if the gateway should be used for this profile -- requires an explicitly configured URL (#878), not just an API key. */
export function gatewayEnabledForProfile(profile?: string): boolean {
  const s = readGatewaySettings();
  if (!s.enabled) return false;
  if (profile && s.disabledProfiles.includes(profile)) return false;
  if (!s.url && !process.env.TDAI_GATEWAY_URL) return false;
  return true;
}

/** Merged context policy for a profile: per-profile override wins over the
 * global default, field by field (#961). Read fresh per call so changing
 * settings takes effect on the next chat with no server restart. */
export function effectiveContextPolicy(profile: string): MemoryContextPolicy {
  const settings = readGatewaySettings();
  const override = settings.contextPolicies[profile] ?? {};
  return {
    budgetChars: override.budgetChars ?? settings.contextPolicy.budgetChars,
    tiers: override.tiers ?? settings.contextPolicy.tiers
  };
}

/** Applies the budget to recalled context deterministically: the gateway
 * returns relevance-ordered context, so highest-relevance-first truncation
 * is keeping the head. Never silent -- the caller records `truncated`. */
export function applyContextBudget(
  context: string,
  policy: MemoryContextPolicy
): { text: string; truncated: boolean } {
  const budget = policy.budgetChars;
  if (!budget || budget <= 0 || context.length <= budget) return { text: context, truncated: false };
  return { text: context.slice(0, budget), truncated: true };
}
