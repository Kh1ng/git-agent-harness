import { CheckCircle2, AlertTriangle, XCircle, HelpCircle, type LucideIcon } from 'lucide-react';

export type StatusTone = 'good' | 'warning' | 'serious' | 'critical' | 'unknown';

const TONE_CLASS: Record<StatusTone, string> = {
  good: 'badge-good',
  warning: 'badge-warning',
  serious: 'badge-serious',
  critical: 'badge-critical',
  unknown: 'badge-unknown'
};

const TONE_ICON: Record<StatusTone, LucideIcon> = {
  good: CheckCircle2,
  warning: AlertTriangle,
  serious: AlertTriangle,
  critical: XCircle,
  unknown: HelpCircle
};

/** Status is never color-only: every badge ships an icon + text label. */
export function StatusBadge({ tone, label }: { tone: StatusTone; label: string }) {
  const Icon = TONE_ICON[tone];
  return (
    <span className={`badge ${TONE_CLASS[tone]}`}>
      <Icon size={12} strokeWidth={2.5} aria-hidden="true" />
      {label}
    </span>
  );
}

/** Maps a `gah sync` classification string to a display tone + label. */
export function classificationTone(classification: string): { tone: StatusTone; label: string } {
  switch (classification) {
    case 'MERGED':
      return { tone: 'good', label: 'Merged' };
    case 'READY_FOR_HUMAN':
      return { tone: 'warning', label: 'Ready for human' };
    case 'CI_FAILED':
      return { tone: 'critical', label: 'CI failed' };
    case 'NEEDS_FIX':
      return { tone: 'critical', label: 'Needs fix' };
    case 'NEEDS_REVIEW':
      return { tone: 'warning', label: 'Needs review' };
    case 'STALE':
      return { tone: 'serious', label: 'Stale' };
    case 'CLOSED_UNMERGED':
      return { tone: 'unknown', label: 'Closed, unmerged' };
    default:
      return { tone: 'unknown', label: classification || 'Unknown' };
  }
}
