import { useEffect, useState } from 'react';
import { formatUpdatedAge } from '../../lib/format.js';

/** Live "Updated Xs/Xm ago" readout so staleness is visible without
 * requiring a manual refresh -- ticks on its own timer so the text keeps
 * advancing even if the underlying page never refetches again. */
export function LastUpdated({ at }: { at: number | null }) {
  const [, forceTick] = useState(0);

  useEffect(() => {
    const timer = window.setInterval(() => forceTick((n) => n + 1), 1000);
    return () => window.clearInterval(timer);
  }, []);

  return (
    <span
      className="text-xs text-muted whitespace-nowrap"
      title={at !== null ? new Date(at).toLocaleString() : undefined}
    >
      Updated {formatUpdatedAge(at)}
    </span>
  );
}
