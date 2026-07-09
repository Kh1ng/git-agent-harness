import type { LucideIcon } from 'lucide-react';

export function StatTile({
  label,
  value,
  icon: Icon,
  hint
}: {
  label: string;
  value: string;
  icon?: LucideIcon;
  hint?: string;
}) {
  return (
    <div className="stat-tile">
      <div className="flex items-center justify-between">
        <span className="stat-tile-label">{label}</span>
        {Icon && <Icon size={14} className="text-muted" aria-hidden="true" />}
      </div>
      <span className="stat-tile-value">{value}</span>
      {hint && <span className="text-xs text-muted">{hint}</span>}
    </div>
  );
}
