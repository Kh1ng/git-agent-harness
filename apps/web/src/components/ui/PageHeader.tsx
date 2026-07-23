import type { ReactNode } from 'react';
import { RefreshCw } from 'lucide-react';
import { LastUpdated } from './LastUpdated.js';

export function PageHeader({
  title,
  description,
  onRefresh,
  refreshing,
  actions,
  lastUpdated
}: {
  title: string;
  description?: string;
  onRefresh?: () => void;
  refreshing?: boolean;
  actions?: ReactNode;
  /** Epoch ms of the freshest (or, for multi-resource pages, oldest-of-the-
   * dependencies) successful fetch backing this page. Pass `undefined` to
   * omit the indicator entirely (page shows no fetched data yet); pass
   * `null` once fetching has started but nothing has resolved. */
  lastUpdated?: number | null;
}) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-4 mb-5">
      <div>
        <h2 className="text-lg font-semibold text-primary">{title}</h2>
        {description && <p className="text-sm text-muted mt-0.5">{description}</p>}
      </div>
      <div className="flex flex-wrap items-center justify-end gap-3">
        {lastUpdated !== undefined && <LastUpdated at={lastUpdated} />}
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
