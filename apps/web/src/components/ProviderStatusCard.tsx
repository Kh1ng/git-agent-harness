import type { ProviderInstance, ProviderStatus } from '@git-agent-harness/contracts';
import { StatusBadge, type StatusTone } from './ui/StatusBadge.js';
import { providerIcon } from '../lib/icons.js';

type ProviderStatusCardProps = {
  provider: ProviderInstance;
  status: ProviderStatus | undefined;
  onClick?: () => void;
};

const STATUS_TONE: Record<ProviderStatus['type'], StatusTone> = {
  unavailable: 'unknown',
  available: 'good',
  authenticated: 'good',
  error: 'critical'
};

export function ProviderStatusCard({ provider, status, onClick }: ProviderStatusCardProps) {
  const Icon = providerIcon(provider.providerKind);
  const tone = status ? STATUS_TONE[status.type] : 'unknown';

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
        <StatusBadge tone={tone} label={status ? status.type : 'unknown'} />
      </div>
    </Wrapper>
  );
}
