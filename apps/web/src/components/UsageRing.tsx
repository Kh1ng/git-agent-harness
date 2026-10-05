import type { SubscriptionUsage } from '../lib/subscriptionUsage.js';
import { usageTone } from '../lib/subscriptionUsage.js';

const TONE_CLASS = { good: 'text-good', warning: 'text-warning', critical: 'text-critical', unknown: 'text-muted' } as const;

/** One arc of the ring: a faint full track and the used share on top of it. */
function Arc({ r, percent, className }: { r: number; percent: number | null; className: string }) {
  const circumference = 2 * Math.PI * r;
  const used = percent === null ? 0 : (Math.min(100, Math.max(0, percent)) / 100) * circumference;
  return (
    <>
      <circle cx="12" cy="12" r={r} fill="none" stroke="currentColor" strokeOpacity="0.18" strokeWidth="1.75" />
      {percent !== null && (
        <circle cx="12" cy="12" r={r} fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" className={className}
          strokeDasharray={`${used} ${circumference}`} transform="rotate(-90 12 12)" />
      )}
    </>
  );
}

/**
 * A subscription as concentric rings: the outer ring is its shortest window
 * (the session), the inner ring the next one (the week), with the provider's
 * initial in the middle. Colour follows how close the tightest window is.
 */
export function UsageRing({ usage, size = 26 }: { usage: SubscriptionUsage; size?: number }) {
  const [outer, inner] = usage.windows.filter((window) => window.usedPercent !== null);
  const tone = usageTone(usage.tightest?.usedPercent ?? null);
  return (
    <span className="relative inline-flex shrink-0 items-center justify-center" style={{ width: size, height: size }} aria-hidden="true">
      <svg className="absolute inset-0" width={size} height={size} viewBox="0 0 24 24">
        <Arc r={11} percent={outer?.usedPercent ?? null} className={TONE_CLASS[tone]} />
        <Arc r={7.5} percent={inner?.usedPercent ?? null} className={TONE_CLASS[usageTone(inner?.usedPercent ?? null)]} />
      </svg>
      <span className="text-[9px] font-semibold uppercase leading-none text-primary">{usage.providerLabel.charAt(0)}</span>
    </span>
  );
}
