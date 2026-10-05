import { useEffect, useState, type ReactNode } from 'react';
import { CircleDot, ExternalLink } from 'lucide-react';
import type { ChatIssueSummary } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import { useUiStore } from '../store/uiStore.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, ErrorState } from '../components/ui/EmptyState.js';
import { ExternalAnchor } from '../components/ExternalAnchor.js';
import { formatAge } from '../lib/format.js';

/**
 * The Git issues sidebar: the current project's open issues, newest first,
 * each opening the work detail drawer that Needs attention uses.
 */
export function IssuesPanel({ renderDetail, onDetailChange }: {
  /** The work detail for an issue, rendered inside this sidebar; `onBack` returns to the list. */
  renderDetail: (workId: string, onBack: () => void) => ReactNode;
  /** Lets the shell widen the sidebar while a detail is open. */
  onDetailChange?: (open: boolean) => void;
}) {
  const [detailWorkId, setDetailWorkId] = useState<string | null>(null);
  useEffect(() => { onDetailChange?.(detailWorkId !== null); return () => onDetailChange?.(false); }, [detailWorkId, onDetailChange]);
  const { profile: wsProfile, reconnectSeq } = useWebSocket();
  const profileOverride = useUiStore((state) => state.profileOverride);
  const profile = profileOverride ?? wsProfile ?? null;
  const [issues, setIssues] = useState<ChatIssueSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [query, setQuery] = useState('');
  const [epoch, setEpoch] = useState(0);

  useEffect(() => {
    if (!profile) return;
    let cancelled = false;
    setLoading(true);
    gahApi.getChatIssues(profile)
      .then(({ issues }) => { if (!cancelled) { setIssues(issues); setError(null); setFetchedAt(Date.now()); } })
      .catch((err) => { if (!cancelled) setError(err instanceof Error ? err.message : String(err)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [profile, reconnectSeq, epoch]);
  const refresh = () => setEpoch((value) => value + 1);
  useWsReconnectRefresh(refresh);

  const terms = query.trim().toLowerCase();
  const visible = (issues ?? [])
    .filter((issue) => !terms || `#${issue.number} ${issue.title} ${issue.labels.join(' ')}`.toLowerCase().includes(terms))
    .sort((a, b) => Date.parse(b.updatedAt ?? '') - Date.parse(a.updatedAt ?? '') || b.number - a.number);

  if (detailWorkId) return <div className="space-y-4">{renderDetail(detailWorkId, () => setDetailWorkId(null))}</div>;

  return (
    <div className="space-y-4">
      <PageHeader title="Git issues" description={profile ? `Open issues on ${profile}` : 'Choose a project first'}
        onRefresh={refresh} refreshing={loading} lastUpdated={fetchedAt} />
      <input type="search" value={query} onChange={(event) => setQuery(event.target.value)} aria-label="Filter issues" placeholder="Number, title or label"
        className="w-full rounded-md border border-subtle bg-raised px-3 py-2 text-sm text-primary placeholder:text-muted focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent" />
      {error && !issues ? (
        <ErrorState message={error} endpoint="/api/manager-chat/issues" onRetry={refresh} />
      ) : issues === null ? (
        <p className="text-sm text-muted">Loading issues…</p>
      ) : visible.length === 0 ? (
        <EmptyState icon={CircleDot} title={issues.length === 0 ? 'No open issues' : 'No issue matches'} description={issues.length === 0 ? 'Nothing is waiting on this project.' : undefined} />
      ) : (
        <ul className="card divide-y divide-subtle" aria-label="Open issues">
          {visible.map((issue) => (
            <li key={issue.number} className="flex items-start gap-2 px-3 py-2">
              <button type="button" onClick={() => setDetailWorkId(`#${issue.number}`)} className="min-w-0 flex-1 text-left hover:bg-white/5 rounded-md -mx-1 px-1 py-0.5">
                <span className="flex items-baseline gap-2">
                  <span className="shrink-0 font-mono text-xs text-accent">#{issue.number}</span>
                  <span className="truncate text-sm text-primary" title={issue.title}>{issue.title}</span>
                </span>
                <span className="block truncate text-[11px] text-muted">
                  {[issue.labels.join(', '), issue.updatedAt ? `updated ${formatAge(issue.updatedAt)}` : null].filter(Boolean).join(' · ')}
                </span>
              </button>
              {issue.url && (
                <ExternalAnchor href={issue.url} className="shrink-0 p-1 text-muted hover:text-primary" aria-label={`Open #${issue.number} on the provider`}>
                  <ExternalLink size={13} aria-hidden="true" />
                </ExternalAnchor>
              )}
            </li>
          ))}
        </ul>
      )}
      {error && issues && <p role="alert" className="text-xs text-critical">Refresh failed: {error}. Showing the last list.</p>}
    </div>
  );
}
