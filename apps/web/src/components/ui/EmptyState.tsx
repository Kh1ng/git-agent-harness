import type { LucideIcon } from 'lucide-react';
import { Loader2, AlertCircle } from 'lucide-react';

export function EmptyState({
  icon: Icon,
  title,
  description
}: {
  icon: LucideIcon;
  title: string;
  description?: string;
}) {
  return (
    <div className="card-padded flex flex-col items-center justify-center gap-2 py-10 text-center">
      <Icon size={28} className="text-muted" aria-hidden="true" />
      <p className="text-sm font-medium text-secondary">{title}</p>
      {description && <p className="text-xs text-muted max-w-sm">{description}</p>}
    </div>
  );
}

export function LoadingState({ label = 'Loading…' }: { label?: string }) {
  return (
    <div className="card-padded flex items-center justify-center gap-2 py-10 text-sm text-muted" role="status">
      <Loader2 size={16} className="animate-spin" aria-hidden="true" />
      {label}
    </div>
  );
}

export function ErrorState({
  message,
  endpoint,
  onRetry
}: {
  message: string;
  endpoint?: string;
  onRetry?: () => void;
}) {
  return (
    <div className="card-padded flex flex-col items-center justify-center gap-2 py-10 text-center border-critical/30">
      <AlertCircle size={24} className="text-critical" aria-hidden="true" />
      <p className="text-sm font-medium text-primary">Failed to load data</p>
      <p className="text-xs text-muted max-w-sm">
        {message}
        {endpoint && <span className="block mt-1 font-mono">{endpoint}</span>}
      </p>
      {onRetry && (
        <button onClick={onRetry} className="btn-secondary mt-2">
          Retry
        </button>
      )}
    </div>
  );
}
