import { useEffect, useRef, useState } from 'react';
import { formatUntil, projectedPercent, usageTone, type SubscriptionUsage, type UsageWindow } from '../lib/subscriptionUsage.js';
import { formatLocalTime } from '../lib/format.js';
import { UsageRing } from './UsageRing.js';

const BAR_CLASS = { good: 'bg-accent', warning: 'bg-warning', critical: 'bg-critical', unknown: 'bg-muted' } as const;

/** A window's name, time to reset and used share, with a thin bar and the pace projection under it. */
export function UsageWindowRow({ window, now, compact = false }: { window: UsageWindow; now: number; compact?: boolean }) {
  const until = formatUntil(window.resetAt, now);
  const projected = projectedPercent(window, now);
  const tone = usageTone(window.usedPercent);
  const id = `usage-${window.key.replace(/[^a-z0-9]+/gi, '-')}`;
  return (
    <div className="flex flex-col">
      <div className="flex items-baseline justify-between gap-2 text-xs">
        <span id={id} className="truncate text-primary">{window.label}</span>
        <span className="flex shrink-0 items-baseline gap-1.5 tabular-nums text-muted">
          {until && <span>{until === 'now' ? 'Resets now' : `Resets in ${until}`}</span>}
          <span className={window.usedPercent === null ? '' : 'text-primary'}>{window.usedPercent === null ? 'no data' : `${Math.round(window.usedPercent)}%`}</span>
        </span>
      </div>
      <div className={`mt-1 overflow-hidden rounded-[3px] bg-overlay/10 ${compact ? 'h-[3px]' : 'h-[4px]'}`} role="progressbar" aria-labelledby={id}
        aria-valuenow={window.usedPercent === null ? undefined : Math.round(window.usedPercent)} aria-valuemin={0} aria-valuemax={100}>
        {window.usedPercent !== null && <div className={`h-full ${BAR_CLASS[tone]} transition-[width]`} style={{ width: `${window.usedPercent}%` }} />}
      </div>
      {!compact && (projected !== null || window.resetAt) && (
        <p className="mt-0.5 text-[11px] tabular-nums text-muted">
          {projected !== null && <>≈{projected}% by reset</>}
          {projected !== null && window.resetAt && ' · '}
          {window.resetAt && <>resets {formatLocalTime(window.resetAt) ?? window.resetAt}</>}
        </p>
      )}
    </div>
  );
}

/** One subscription's windows, headed by the tightest one in plain words. */
export function SubscriptionDetail({ usage, now }: { usage: SubscriptionUsage; now: number }) {
  const tight = usage.tightest;
  const until = tight ? formatUntil(tight.resetAt, now) : null;
  return (
    <div className="flex flex-col gap-2 px-3 py-2">
      <div className="flex items-center gap-2">
        <UsageRing usage={usage} size={22} />
        <div className="min-w-0">
          <p className="truncate text-sm font-semibold text-primary">{usage.providerLabel} <span className="font-normal text-muted">· {usage.id}</span></p>
          {usage.model && <p className="truncate text-[11px] text-muted">{usage.model}</p>}
        </div>
      </div>
      {tight && tight.usedPercent !== null ? (
        <p className="rounded bg-overlay/5 px-2 py-1 text-xs text-primary">
          <span className="font-medium">{Math.round(tight.usedPercent)}% of {tight.label.toLowerCase()} used.</span>
          {until && <> Resets in {until}.</>}
          {usage.windows.length > 1 && <span className="block text-muted">That is the tightest limit right now.</span>}
        </p>
      ) : (
        <p className="text-xs text-muted">{usage.eligible ? 'No usage figures from this provider yet.' : usage.reason ?? 'Unavailable'}</p>
      )}
      {!usage.eligible && usage.reason && tight && <p className="text-xs text-warning">{usage.reason}</p>}
      {usage.windows.map((window) => <UsageWindowRow key={window.key} window={window} now={now} />)}
      {usage.costUsd !== null && <p className="text-[11px] tabular-nums text-muted">Spend this period ${usage.costUsd.toFixed(2)}</p>}
    </div>
  );
}

/**
 * Subscription chips at the right of the navbar: a ring per subscription
 * that fills as its limits are used. Hovering or clicking one opens that
 * subscription's windows; the ring's colour says how close it is.
 */
export function SubscriptionUsageMenu({ subscriptions, busy, onOpenQuota }: { subscriptions: SubscriptionUsage[]; /** Subscriptions with a job running now. */ busy?: Set<string>; onOpenQuota: () => void }) {
  const [openId, setOpenId] = useState<string | null>(null);
  const [pinned, setPinned] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const menu = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!openId) return;
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [openId]);

  useEffect(() => {
    if (!openId) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent ? event.key === 'Escape' : !menu.current?.contains(event.target as Node)) { setOpenId(null); setPinned(false); }
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => { document.removeEventListener('mousedown', close); document.removeEventListener('keydown', close); };
  }, [openId]);

  if (subscriptions.length === 0) return null;
  const open = subscriptions.find((usage) => usage.id === openId) ?? null;
  return (
    <div className="relative flex items-center" ref={menu} onMouseLeave={() => { if (!pinned) setOpenId(null); }}>
      <div className="flex items-center gap-0.5" role="group" aria-label="Subscription usage">
        {subscriptions.map((usage) => {
          const tight = usage.tightest;
          const working = busy?.has(usage.id) ?? false;
          const label = `${usage.providerLabel} ${usage.id} usage${tight && tight.usedPercent !== null ? `: ${Math.round(tight.usedPercent)}% of ${tight.label.toLowerCase()} used` : ''}${working ? ', working now' : ''}`;
          return (
            <button key={usage.id} type="button" aria-label={label} aria-expanded={openId === usage.id} aria-haspopup="true"
              onMouseEnter={() => { if (!pinned) { setOpenId(usage.id); setNow(Date.now()); } }}
              onClick={() => { setPinned(openId !== usage.id || !pinned); setOpenId(usage.id); setNow(Date.now()); }}
              className={`flex h-9 w-9 items-center justify-center rounded-md hover:bg-overlay/5 ${openId === usage.id ? 'bg-overlay/5' : ''}`}>
              <UsageRing usage={usage} working={working} />
            </button>
          );
        })}
      </div>
      {open && (
        <div className="absolute right-0 top-full z-40 mt-1 w-80 max-w-[calc(100vw-1.5rem)] rounded-lg border border-subtle bg-raised shadow-xl" role="dialog" aria-label={`${open.providerLabel} usage`}>
          <SubscriptionDetail usage={open} now={now} />
          <button type="button" className="w-full border-t border-subtle px-3 py-2 text-left text-xs font-medium text-accent hover:underline" onClick={() => { setOpenId(null); setPinned(false); onOpenQuota(); }}>
            See all subscriptions
          </button>
        </div>
      )}
    </div>
  );
}
