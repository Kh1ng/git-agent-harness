import type { ReactNode } from 'react';
import { RefreshCw } from 'lucide-react';

export function PageHeader({
  title,
  description,
  onRefresh,
  refreshing,
  actions
}: {
  title: string;
  description?: string;
  onRefresh?: () => void;
  refreshing?: boolean;
  actions?: ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-4 mb-5">
      <div>
        <h2 className="text-lg font-semibold text-primary">{title}</h2>
        {description && <p className="text-sm text-muted mt-0.5">{description}</p>}
      </div>
      <div className="flex items-center gap-2 shrink-0">
        {actions}
        {onRefresh && (
          <button onClick={onRefresh} disabled={refreshing} className="btn-secondary" aria-label="Refresh">
            <RefreshCw size={14} className={refreshing ? 'animate-spin' : ''} aria-hidden="true" />
            <span className="hidden sm:inline">Refresh</span>
          </button>
        )}
      </div>
    </div>
  );
}
