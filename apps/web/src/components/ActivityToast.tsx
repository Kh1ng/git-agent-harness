import { Bell, X } from 'lucide-react';
import type { ActivityEvent } from '@git-agent-harness/contracts';

export function ActivityToast({ event, onOpen, onDismiss }: {
  event: ActivityEvent;
  onOpen: () => void;
  onDismiss: () => void;
}) {
  return (
    <aside
      className="fixed inset-x-4 bottom-[max(1rem,env(safe-area-inset-bottom))] z-40 ml-auto max-w-sm rounded-lg bg-raised p-4 shadow-xl sm:inset-x-auto sm:right-5"
      aria-live="polite"
      aria-label="New activity"
    >
      <div className="flex items-start gap-3">
        <Bell size={17} className="mt-0.5 shrink-0 text-accent" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold text-primary">{event.title}</p>
          <p className="mt-1 line-clamp-2 text-xs text-secondary">{event.message}</p>
          <button type="button" className="mt-2 text-xs font-medium text-accent hover:underline" onClick={onOpen}>
            View activity
          </button>
        </div>
        <button type="button" className="-m-2 min-h-11 min-w-11 p-2 text-muted hover:text-primary" onClick={onDismiss} aria-label="Dismiss notification">
          <X size={16} aria-hidden="true" />
        </button>
      </div>
    </aside>
  );
}
