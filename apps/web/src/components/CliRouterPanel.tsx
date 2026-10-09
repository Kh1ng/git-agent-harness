import { useState, useEffect, useCallback, useRef } from 'react';
import { Wifi, WifiOff, Settings, RefreshCw, ChevronDown, ChevronRight, Copy, Check, Eye, EyeOff, Gauge } from 'lucide-react';
import { StatusBadge } from './ui/StatusBadge.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { formatRemaining, formatLocalTime, isStale } from '../lib/format.js';
import { cliRouterApi } from '../api/client.js';
import type { CliRouterSnapshot, CliRouterSettingsPayload } from '../api/client.js';

const ROUTER_REFRESH_MS = 60_000;
/** Group account identities, never models or quota windows, into provider filters. */
function accountProviders(accounts: CliRouterSnapshot['accounts']): string[] {
  return [...new Set(accounts.map(account => account.provider))];
}

/** Keep the same masked identity in account rows and summary accessibility labels. */
function accountDisplayName(account: CliRouterSnapshot['accounts'][number], showLabel: boolean): string {
  return showLabel ? (account.label || account.name)
    : account.name.includes('@') ? `${account.provider} · ${account.id.slice(0, 8)}` : account.name;
}

function remainingPercent(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : null;
}

function quotaTone(remaining: number | null): string {
  return remaining === null ? 'text-muted' : remaining < 20 ? 'text-critical' : remaining < 60 ? 'text-warning' : 'text-good';
}

/** Feedback message auto-cleared after display. */
interface Feedback {
  kind: 'success' | 'error';
  text: string;
}

function useFeedback(clearMs = 4000): [Feedback | null, (fb: Feedback) => void] {
  const [fb, setFb] = useState<Feedback | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const set = useCallback((f: Feedback) => {
    clearTimeout(timer.current);
    setFb(f);
    if (f.kind === 'success') timer.current = setTimeout(() => setFb(null), clearMs);
  }, [clearMs]);
  useEffect(() => () => clearTimeout(timer.current), []);
  return [fb, set];
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

// ---------------------------------------------------------------------------
// Main panel
// ---------------------------------------------------------------------------

export function CliRouterPanel() {
  const [snapshot, setSnapshot] = useState<CliRouterSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const fetchSequence = useRef(0);

  const fetchSnapshot = useCallback(async () => {
    const sequence = ++fetchSequence.current;
    try {
      const data = await cliRouterApi.getSnapshot();
      if (sequence !== fetchSequence.current) return;
      // A failed connection does not remove accounts. Keep the last inventory
      // until a successful read confirms it, including a confirmed empty list.
      setSnapshot(previous => data.status === 'unavailable' && data.accounts.length === 0 && previous
        ? { ...data, accounts: previous.accounts }
        : data);
      setError(null);
    } catch (err) {
      if (sequence !== fetchSequence.current) return;
      setError(errorText(err));
    } finally {
      if (sequence === fetchSequence.current) setLoading(false);
    }
  }, []);

  useEffect(() => { fetchSnapshot(); }, [fetchSnapshot]);
  useAutoRefresh(fetchSnapshot, ROUTER_REFRESH_MS);

  if (loading && !snapshot) {
    return (
      <section className="card-padded" data-testid="cli-router-panel">
        <h3 className="text-sm font-semibold text-primary mb-3">CLI Router</h3>
        <p className="text-xs text-muted" role="status">Loading router status…</p>
      </section>
    );
  }

  if (error && !snapshot) {
    return (
      <section className="card-padded" data-testid="cli-router-panel">
        <h3 className="text-sm font-semibold text-primary mb-3">CLI Router</h3>
        <div role="alert" className="text-xs text-critical">{error}</div>
        <button onClick={fetchSnapshot} className="btn-secondary mt-2">Retry</button>
      </section>
    );
  }

  return (
    <section className="space-y-4 [&_button]:min-h-11 sm:[&_button]:min-h-0" data-testid="cli-router-panel">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          {snapshot?.status === 'connected'
            ? <Wifi size={16} className="text-good" aria-hidden="true" />
            : <WifiOff size={16} className="text-muted" aria-hidden="true" />}
          <h3 className="text-sm font-semibold text-primary">CLI Router</h3>
          <StatusBadge
            tone={error ? 'warning' : snapshot?.status === 'connected' ? 'good' : snapshot?.status === 'unavailable' ? 'critical' : 'unknown'}
            label={error ? 'Stale' : snapshot?.status ?? 'unknown'}
          />
        </div>
        <button
          onClick={fetchSnapshot}
          className="btn-secondary"
          aria-label="Refresh router"
        >
          <RefreshCw size={14} aria-hidden="true" />
          <span className="hidden sm:inline">Refresh</span>
        </button>
      </div>

      {error && <p role="alert" className="text-xs text-critical">Latest refresh failed. Showing the earlier snapshot. {error}</p>}
      {snapshot?.status === 'unavailable' && <p role="alert" className="text-xs text-critical">Router unavailable. Account inventory and quota readings may be incomplete or from an earlier check. Refresh to check the connection.</p>}
      {snapshot && (snapshot.status === 'connected' || snapshot.accounts.length > 0) && (
        <AccountList accounts={snapshot.accounts} onChanged={updated => {
          if (!updated) { void fetchSnapshot(); return; }
          // Account refresh can return fresh quotas while models/routing are
          // unavailable. Use that response rather than discard it in a reread.
          fetchSequence.current += 1;
          setSnapshot(updated);
          setError(null);
        }} />
      )}

      <details className="quota-settings" open={snapshot?.status === 'unconfigured'}>
        <summary className="text-sm text-secondary cursor-pointer py-3">Router settings and models</summary>
        <div className="space-y-3 pb-3">
          <ConnectionSettings
            settings={snapshot?.settings ?? { url: null, hasApiKey: false, hasManagementKey: false }}
            status={snapshot?.status ?? 'unconfigured'}
            onSaved={fetchSnapshot}
          />
          {snapshot?.status === 'connected' && (
            <>
              <RoutingControls strategy={snapshot.strategy} sessionAffinity={snapshot.sessionAffinity} onChanged={fetchSnapshot} />
              <ModelList models={snapshot.models} />
            </>
          )}
        </div>
      </details>

      {snapshot?.status === 'unconfigured' && (
        <div className="card-padded text-xs text-muted space-y-2">
          <p>The CLI router is not configured. To get started:</p>
          <ol className="list-decimal pl-5 space-y-1">
            <li>Run the setup script: <code className="text-secondary">scripts/setup-cli-router.py</code></li>
            <li>Add OAuth accounts through the upstream management console or tunnel</li>
            <li>Enter the router URL and keys in the connection form above</li>
          </ol>
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Connection settings
// ---------------------------------------------------------------------------

function ConnectionSettings({
  settings,
  status,
  onSaved,
}: {
  settings: CliRouterSnapshot['settings'];
  status: CliRouterSnapshot['status'];
  onSaved: () => void;
}) {
  const [expanded, setExpanded] = useState(status === 'unconfigured');
  const [url, setUrl] = useState(settings.url ?? '');
  const [apiKey, setApiKey] = useState('');
  const [managementKey, setManagementKey] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [feedback, setFeedback] = useFeedback();

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    if (submitting) return; // prevent duplicate submits
    setSubmitting(true);
    try {
      const body: CliRouterSettingsPayload = { url: url.trim() };
      if (apiKey) body.apiKey = apiKey;
      if (managementKey) body.managementKey = managementKey;
      await cliRouterApi.saveSettings(body);
      setFeedback({ kind: 'success', text: 'Connection saved' });
      setApiKey('');
      setManagementKey('');
      onSaved();
    } catch (err) {
      setFeedback({ kind: 'error', text: errorText(err) });
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="card-padded">
      <button
        type="button"
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-2 w-full text-left text-sm font-medium text-primary"
        aria-expanded={expanded}
      >
        {expanded ? <ChevronDown size={14} aria-hidden="true" /> : <ChevronRight size={14} aria-hidden="true" />}
        <Settings size={14} className="text-muted" aria-hidden="true" />
        Connection settings
        {settings.url && <span className="text-xs text-muted font-normal ml-auto truncate max-w-[200px]">{settings.url}</span>}
      </button>

      {expanded && (
        <form onSubmit={handleSave} className="mt-4 space-y-3">
          <label className="block">
            <span className="text-xs text-muted block mb-1">Router URL</span>
            <input
              type="url"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://router.example.com"
              className="input w-full"
              required
              autoComplete="url"
            />
          </label>
          <label className="block">
            <span className="text-xs text-muted block mb-1">
              API Key {settings.hasApiKey && <span className="text-good">(saved)</span>}
            </span>
            <input
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder={settings.hasApiKey ? '••••••••' : 'Required for initial setup'}
              className="input w-full"
              autoComplete="new-password"
            />
          </label>
          <label className="block">
            <span className="text-xs text-muted block mb-1">
              Management Key {settings.hasManagementKey && <span className="text-good">(saved)</span>}
            </span>
            <input
              type="password"
              value={managementKey}
              onChange={(e) => setManagementKey(e.target.value)}
              placeholder={settings.hasManagementKey ? '••••••••' : 'Required for initial setup'}
              className="input w-full"
              autoComplete="new-password"
            />
          </label>

          {feedback && (
            <p role={feedback.kind === 'error' ? 'alert' : 'status'} className={`text-xs ${feedback.kind === 'error' ? 'text-critical' : 'text-good'}`}>
              {feedback.text}
            </p>
          )}

          <button
            type="submit"
            disabled={submitting || !url.trim()}
            className="btn-primary"
          >
            {submitting ? 'Saving…' : 'Save connection'}
          </button>
        </form>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Routing controls (strategy + session affinity)
// ---------------------------------------------------------------------------

function RoutingControls({
  strategy,
  sessionAffinity,
  onChanged,
}: {
  strategy: CliRouterSnapshot['strategy'];
  sessionAffinity: boolean;
  onChanged: () => void;
}) {
  const [submitting, setSubmitting] = useState(false);
  const [feedback, setFeedback] = useFeedback();
  /** Tracks which part of the two-step routing write failed, for partial error recovery. */
  const [partialError, setPartialError] = useState<string | null>(null);

  const handleChange = async (newStrategy: CliRouterSnapshot['strategy'], newAffinity: boolean) => {
    if (submitting) return;
    setSubmitting(true);
    setPartialError(null);
    try {
      await cliRouterApi.setRouting({ strategy: newStrategy, sessionAffinity: newAffinity });
      setFeedback({ kind: 'success', text: 'Routing updated' });
      onChanged();
    } catch (err) {
      const msg = errorText(err);
      setPartialError(msg);
      setFeedback({ kind: 'error', text: msg });
      // Still refresh to show any partial state change
      onChanged();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="card-padded">
      <h4 className="text-[11px] uppercase tracking-wide text-muted mb-3">Routing</h4>
      <div className="flex flex-wrap items-center gap-4">
        <label className="flex items-center gap-2 text-xs text-secondary">
          <span>Strategy</span>
          <select
            value={strategy}
            onChange={(e) => handleChange(e.target.value as CliRouterSnapshot['strategy'], sessionAffinity)}
            disabled={submitting}
            className="input py-1"
          >
            <option value="round-robin">Round Robin</option>
            <option value="fill-first">Fill First</option>
            <option value="weighted-round-robin">Weighted Round Robin</option>
          </select>
        </label>

        <label className="flex items-center gap-2 text-xs text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={sessionAffinity}
            onChange={(e) => handleChange(strategy, e.target.checked)}
            disabled={submitting}
          />
          Session affinity
        </label>
      </div>

      {feedback && (
        <p role={feedback.kind === 'error' ? 'alert' : 'status'} className={`text-xs mt-2 ${feedback.kind === 'error' ? 'text-critical' : 'text-good'}`}>
          {feedback.text}
        </p>
      )}
      {partialError && !feedback && (
        <p role="alert" className="text-xs mt-2 text-critical">
          Routing write partially failed: {partialError}. The displayed state may be stale.
        </p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Account list with provider tabs
// ---------------------------------------------------------------------------

function AccountList({
  accounts,
  onChanged,
}: {
  accounts: CliRouterSnapshot['accounts'];
  onChanged: (snapshot?: CliRouterSnapshot) => void;
}) {
  const [providerFilter, setProviderFilter] = useState('All');
  const [showLabels, setShowLabels] = useState(false);
  const providers = accountProviders(accounts);
  const activeFilter = providers.includes(providerFilter) ? providerFilter : 'All';
  const filteredProviders = activeFilter === 'All' ? providers : [activeFilter];

  return (
    <div className="quota-accounts">
      <div className="flex flex-wrap items-center justify-between gap-3 mb-4">
        <p className="text-sm text-muted tabular-nums">
          <span className="text-primary">{accounts.length} {accounts.length === 1 ? 'account' : 'accounts'}</span>
          <span className="mx-2">·</span>
          <span className="text-good">{accounts.filter(account => !account.disabled && !account.unavailable).length} active</span>
        </p>
        <label className="btn-secondary cursor-pointer focus-within:ring-2 focus-within:ring-accent">
          {showLabels ? <Eye size={14} aria-hidden="true" /> : <EyeOff size={14} aria-hidden="true" />}
          <input type="checkbox" checked={showLabels} onChange={e => setShowLabels(e.target.checked)} className="sr-only" />
          <span>{showLabels ? 'Labels visible' : 'Show labels'}</span>
        </label>
      </div>

      <div className="quota-provider-tabs" role="group" aria-label="Filter accounts by provider">
        {['All', ...providers].map(provider => (
          <button key={provider} aria-pressed={activeFilter === provider} onClick={() => setProviderFilter(provider)} className="quota-provider-tab">
            <Gauge size={15} aria-hidden="true" />
            {provider}
            <span className="quota-provider-count">({provider === 'All' ? accounts.length : accounts.filter(account => account.provider === provider).length})</span>
          </button>
        ))}
      </div>

      {accounts.length === 0 ? <p className="text-sm text-muted py-6">No router accounts configured.</p> : (
        <>
          <div className="quota-summary-band" aria-label="Provider quota summaries">
            {filteredProviders.map(provider => {
              const providerAccounts = accounts.filter(account => account.provider === provider);
              // Each label remains its own allowance. Percentages from different windows
              // cannot be added, and unknown readings never count as a zero balance.
              const labels = [...new Set(providerAccounts.flatMap(account => account.quotas.map(quota => quota.label)))];
              return (
                <section key={provider} className="quota-provider-summary" data-testid={`quota-summary-${provider}`}>
                  <div className="flex items-center justify-between gap-3 mb-4">
                    <h4 className="text-sm font-semibold text-primary">{provider}</h4>
                    <span className="text-xs text-muted">{providerAccounts.length} {providerAccounts.length === 1 ? 'account' : 'accounts'}</span>
                  </div>
                  {labels.length === 0 ? <p className="text-sm text-muted">Quota not checked</p> : labels.map((label, labelIndex) => {
                    const readings = providerAccounts.map(account => account.quotas.find(quota => quota.label === label));
                    const known = readings.map(quota => remainingPercent(quota?.remainingPercent)).filter((value): value is number => value !== null);
                    const total = known.reduce((sum, value) => sum + value, 0);
                    if (labelIndex > 0) {
                      return <p key={label} className="quota-summary-secondary text-xs text-muted"><span>{label}</span><span className="text-secondary tabular-nums">{known.length ? `${Math.round(total * 10) / 10}% of ${known.length * 100}%` : 'Unknown'} · {known.length}/{providerAccounts.length} reported{readings.some(quota => isStale(quota?.observedAt)) ? ' · Stale' : ''}</span></p>;
                    }
                    const resetTimes = readings.map(quota => quota?.resetAt).filter((value): value is string => !!value && Number.isFinite(Date.parse(value)) && Date.parse(value) > Date.now()).sort((left, right) => Date.parse(left) - Date.parse(right));
                    const nextReset = resetTimes[0];
                    return (
                      <div key={label} className="quota-summary-window">
                        <p className="text-xs text-muted mb-1">{label} · remaining</p>
                        <div className="flex items-baseline gap-2 tabular-nums">
                          <span className="text-2xl font-semibold text-primary">{known.length ? `${Math.round(total * 10) / 10}%` : 'Unknown'}</span>
                          {known.length > 0 && <span className="text-xs text-muted">of {known.length * 100}%</span>}
                        </div>
                        <div className="flex gap-1 my-2" aria-label={`${label}: individual account allowances`}>
                          {readings.map((quota, index) => {
                            const remaining = remainingPercent(quota?.remainingPercent);
                            return remaining === null ? <span key={providerAccounts[index].id} className="quota-unknown-segment flex-1" title={`${accountDisplayName(providerAccounts[index], showLabels)}: unknown`} /> : (
                              <progress key={providerAccounts[index].id} className={`usage-progress router-quota-progress flex-1 ${quotaTone(remaining)}`} max={100} value={remaining} aria-label={`${accountDisplayName(providerAccounts[index], showLabels)}, ${label}: ${remaining}% remaining`} />
                            );
                          })}
                        </div>
                        <p className="text-xs text-muted">{known.length}/{providerAccounts.length} {providerAccounts.length === 1 ? 'account' : 'accounts'} reported{readings.some(quota => isStale(quota?.observedAt)) ? ' · Stale readings' : ''}</p>
                        <p className="text-xs text-muted mt-1">{nextReset ? `Next reset ${formatRemaining(nextReset) ? `in ${formatRemaining(nextReset)}` : formatLocalTime(nextReset)}` : 'No reset time reported'}</p>
                      </div>
                    );
                  })}
                </section>
              );
            })}
          </div>
          {accounts.some(account => account.quotas.some(quota => remainingPercent(quota.remainingPercent) !== null)) && (
            <p className="text-xs text-muted mt-2">Totals add reported account percentages within each window; they do not compare token capacities.</p>
          )}
          <div className="quota-provider-ledgers">
            {filteredProviders.map(provider => (
              <section key={provider}>
                <h4 className="text-sm font-semibold text-primary mb-2">{provider} <span className="text-muted font-normal ml-2">{accounts.filter(account => account.provider === provider).length}</span></h4>
                {accounts.filter(account => account.provider === provider).map(account => (
                  <AccountRow key={account.id} account={account} showLabel={showLabels} onChanged={onChanged} />
                ))}
              </section>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Single account row with actions
// ---------------------------------------------------------------------------

function AccountRow({
  account,
  showLabel,
  onChanged,
}: {
  account: CliRouterSnapshot['accounts'][number];
  showLabel: boolean;
  onChanged: (snapshot?: CliRouterSnapshot) => void;
}) {
  const [actionPending, setActionPending] = useState<'toggle' | 'refresh' | null>(null);
  const [feedback, setFeedback] = useFeedback();
  const displayName = accountDisplayName(account, showLabel);

  const handleToggle = async () => {
    if (actionPending) return;
    setActionPending('toggle');
    try {
      await cliRouterApi.setAccountStatus({ id: account.id, disabled: !account.disabled });
      setFeedback({ kind: 'success', text: account.disabled ? 'Enabled' : 'Paused' });
      onChanged();
    } catch (err) {
      setFeedback({ kind: 'error', text: errorText(err) });
    } finally {
      setActionPending(null);
    }
  };

  const handleRefresh = async () => {
    if (actionPending) return;
    setActionPending('refresh');
    try {
      const updated = await cliRouterApi.refreshAccount({ id: account.id });
      const quotaError = updated.accounts.find(row => row.id === account.id)?.quotaError;
      setFeedback(quotaError ? { kind: 'error', text: quotaError } : { kind: 'success', text: 'Quota refreshed' });
      onChanged(updated);
    } catch (err) {
      setFeedback({ kind: 'error', text: errorText(err) });
    } finally {
      setActionPending(null);
    }
  };

  const tone = account.unavailable ? 'critical'
    : account.disabled ? 'warning'
    : 'good';
  const statusLabel = account.unavailable ? 'Unavailable'
    : account.disabled ? 'Paused'
    : 'Active';

  return (
    <div className="quota-account-row" data-testid={`account-${account.id}`}>
      <div className="min-w-0 quota-account-identity">
        <p className="text-sm font-medium text-primary break-all">{displayName}</p>
        {showLabel && account.label && account.label !== account.name && <p className="text-xs text-muted mt-1 break-all">{account.name}</p>}
        <div className="mt-2"><StatusBadge tone={tone} label={statusLabel} /></div>
        {account.unavailable && account.resetAt && <p className="text-xs text-muted mt-2">Available again {formatRemaining(account.resetAt) ? `in ${formatRemaining(account.resetAt)}` : (formatLocalTime(account.resetAt) ?? account.resetAt)}</p>}
      </div>
      <div className="quota-account-windows">
        {account.quotas.map((quota, index) => {
          const remaining = remainingPercent(quota.remainingPercent);
          const reset = formatRemaining(quota.resetAt);
          return (
            <div key={index} className="min-w-0 text-xs text-secondary">
              <div className="flex items-start justify-between gap-2 mb-2">
                <span>{quota.label}</span>
                <span className="font-semibold text-primary tabular-nums whitespace-nowrap">{remaining === null ? 'Unknown' : `${remaining}% remaining`}</span>
              </div>
              {remaining !== null && <progress className={`usage-progress router-quota-progress ${quotaTone(remaining)}`} max={100} value={remaining} aria-label={`${quota.label}: ${remaining}% remaining`} />}
              <p className="text-xs text-muted mt-2">{reset ? `Resets in ${reset}` : quota.resetAt ? `Resets ${formatLocalTime(quota.resetAt) ?? quota.resetAt}` : 'No reset time reported'}</p>
              <p className="text-xs text-muted mt-1">{quota.observedAt ? `Checked ${formatLocalTime(quota.observedAt)}` : 'No observation time'}{isStale(quota.observedAt) && <span className="text-serious"> · Stale</span>}</p>
            </div>
          );
        })}
        {account.quotas.length === 0 && <p className="text-xs text-muted">Quota not checked. Refresh this account to read its remaining allowance.</p>}
      </div>
      <div className="quota-account-actions">
        <button onClick={handleToggle} disabled={actionPending !== null} className="btn-secondary text-xs" aria-label={account.disabled ? `Enable ${displayName}` : `Pause ${displayName}`}>
          {actionPending === 'toggle' ? '…' : account.disabled ? 'Enable' : 'Pause'}
        </button>
        <button onClick={handleRefresh} disabled={actionPending !== null} className="btn-secondary text-xs" aria-label={`Refresh quota for ${displayName}`}>
          <RefreshCw size={13} className={actionPending === 'refresh' ? 'animate-spin' : ''} aria-hidden="true" />
          Refresh quota
        </button>
      </div>
      {account.quotaError && <p className="quota-row-feedback text-xs text-critical">{account.quotaError}</p>}
      {feedback && <p className={`quota-row-feedback text-xs ${feedback.kind === 'error' ? 'text-critical' : 'text-good'}`} role={feedback.kind === 'error' ? 'alert' : 'status'}>{feedback.text}</p>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Model list with copy
// ---------------------------------------------------------------------------

function ModelList({ models }: { models: CliRouterSnapshot['models'] }) {
  const [copied, setCopied] = useState<string | null>(null);

  const copyId = (id: string) => {
    const gahId = `gah-router/${id}`;
    navigator.clipboard.writeText(gahId).then(() => {
      setCopied(gahId);
      setTimeout(() => setCopied(null), 2000);
    }).catch(() => { /* clipboard unavailable */ });
  };

  if (models.length === 0) return null;

  return (
    <div className="card-padded">
      <h4 className="text-[11px] uppercase tracking-wide text-muted mb-3">
        Models ({models.length})
      </h4>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-1.5">
        {models.map((m) => {
          const gahId = `gah-router/${m.id}`;
          return (
            <div
              key={m.id}
              className="flex items-center justify-between gap-2 rounded-md px-2.5 py-1.5 text-xs text-secondary hover:bg-overlay/5 group"
            >
              <div className="min-w-0">
                <span className="font-mono text-primary truncate block">{gahId}</span>
                <span className="text-muted">{m.ownedBy}</span>
              </div>
              <button
                onClick={() => copyId(m.id)}
                className="btn-secondary !min-h-11 !min-w-11 !px-2 sm:opacity-0 group-hover:opacity-100 focus:opacity-100 flex-shrink-0"
                aria-label={`Copy ${gahId}`}
              >
                {copied === gahId
                  ? <Check size={12} className="text-good" aria-hidden="true" />
                  : <Copy size={12} className="text-muted" aria-hidden="true" />}
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
