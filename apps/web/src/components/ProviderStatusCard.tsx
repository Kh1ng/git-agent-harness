import type { ProviderInstance, ProviderStatus, ProviderKind } from '@git-agent-harness/contracts';
import { StatusBadge, type StatusTone } from './ui/StatusBadge.js';
import { providerIcon } from '../lib/icons.js';

type ProviderStatusCardProps = {
  provider: ProviderInstance;
  status: ProviderStatus | undefined;
  // TICKET-157: maps a backend name to whether it is configured for the
  // active profile (real implementation + profile-level config present).
  backendConfigured?: Record<string, boolean>;
  onClick?: () => void;
};

const STATUS_TONE: Record<ProviderStatus['type'], StatusTone> = {
  unavailable: 'unknown',
  available: 'good',
  authenticated: 'good',
  error: 'critical',
  not_implemented: 'unknown'
};

// Backends that have a real Rust implementation (vs. pure UI scaffolding
// like `grok`/`cursor`, which are never wired through the harness).
const IMPLEMENTED_BACKENDS = new Set<ProviderKind>([
  'codex',
  'claude',
  'agy',
  'vibe',
  'opencode',
  'openhands'
]);

export function ProviderStatusCard({ provider, status, backendConfigured, onClick }: ProviderStatusCardProps) {
  const Icon = providerIcon(provider.providerKind);

  // Determine the effective status: a backend with a real implementation
  // but no profile-level config should read as "not configured for this
  // profile" rather than the generic "available" reactive default.
  let effectiveStatus = status;
  if (
    status?.type === 'available' &&
    IMPLEMENTED_BACKENDS.has(provider.providerKind) &&
    backendConfigured &&
    backendConfigured[provider.providerKind] === false
  ) {
    effectiveStatus = { type: 'unavailable', reason: 'Not configured for this profile' };
  }

  const tone = effectiveStatus ? STATUS_TONE[effectiveStatus.type] : 'unknown';
  const label =
    effectiveStatus?.type === 'unavailable' && effectiveStatus.reason
      ? effectiveStatus.reason
      : effectiveStatus
        ? effectiveStatus.type
        : 'unknown';

  const Wrapper = onClick ? 'button' : 'div';

  return (
    <Wrapper
      onClick={onClick}
      className={`card-padded flex items-center justify-between w-full text-left ${onClick ? 'hover:border-accent/40' : ''}`}
    >
      <div className="flex items-center gap-3 min-w-0">
        <Icon size={18} className="text-muted shrink-0" aria-hidden="true" />
        <div className="min-w-0">
          <p className="text-sm font-medium text-primary truncate">{provider.name}</p>
          <p className="text-xs text-muted">{provider.providerKind}</p>
        </div>
      </div>

      <div className="flex items-center gap-2 shrink-0">
        {status?.type === 'authenticated' && status.userId && (
          <span className="text-xs text-muted hidden sm:inline">{status.userId}</span>
        )}
        <StatusBadge tone={tone} label={label} />
      </div>
    </Wrapper>
  );
}
