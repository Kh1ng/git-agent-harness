import { useEffect, useState, type ReactNode } from 'react';
import { CircleDot, ExternalLink, MessageSquare } from 'lucide-react';
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
export function IssuesPanel({ renderDetail, detailWorkId, onSelectWork, onDetailChange, onOpenChat }: {
  /** The work detail for an issue, rendered inside this sidebar; `onBack` returns to the list. */
  renderDetail: (workId: string, onBack: () => void) => ReactNode;
  /** The work shown in detail; pages open their work here too. */
  detailWorkId: string | null;
  onSelectWork: (workId: string | null) => void;
  /** Lets the shell widen the sidebar while a detail is open. */
  onDetailChange?: (open: boolean) => void;
  /** Opens the chat once an issue's conversation has been started. */
  onOpenChat: () => void;
}) {
  const openChatSession = useUiStore((state) => state.openChatSession);
  const [starting, setStarting] = useState<number | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
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

  /** Start a chat seeded with the issue on the profile's default chat backend, then open it. */
  const startChat = async (number: number) => {
    if (!profile || starting !== null) return;
    setStarting(number);
    setStartError(null);
    try {
      const settings = await gahApi.getManagerChatSettings();
      const backend = settings.profileOverrides[profile] ?? settings.defaultBackend;
      const { session } = await gahApi.startChatFromIssue(profile, number, backend, null);
      openChatSession(profile, session.id);
      onOpenChat();
    } catch (err) {
      setStartError(err instanceof Error ? err.message : String(err));
    } finally {
      setStarting(null);
    }
  };
  useWsReconnectRefresh(refresh);

  const terms = query.trim().toLowerCase();
  const visible = (issues ?? [])
    .filter((issue) => !terms || `#${issue.number} ${issue.title} ${issue.labels.join(' ')}`.toLowerCase().includes(terms))
    .sort((a, b) => Date.parse(b.updatedAt ?? '') - Date.parse(a.updatedAt ?? '') || b.number - a.number);

  if (detailWorkId) return <div className="space-y-4">{renderDetail(detailWorkId, () => onSelectWork(null))}</div>;

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
              <button type="button" onClick={() => onSelectWork(`#${issue.number}`)} className="min-w-0 flex-1 text-left hover:bg-overlay/5 rounded-md -mx-1 px-1 py-0.5">
                <span className="flex items-baseline gap-2">
                  <span className="shrink-0 font-mono text-xs text-accent">#{issue.number}</span>
                  <span className="truncate text-sm text-primary" title={issue.title}>{issue.title}</span>
                </span>
                <span className="block truncate text-[11px] text-muted">
                  {[issue.labels.join(', '), issue.updatedAt ? `updated ${formatAge(issue.updatedAt)}` : null].filter(Boolean).join(' · ')}
                </span>
              </button>
              <button type="button" onClick={() => void startChat(issue.number)} disabled={starting !== null}
                className="shrink-0 p-1 text-muted hover:text-primary disabled:opacity-50" aria-label={`Start a chat on #${issue.number}`} title="Start a chat seeded with this issue">
                <MessageSquare size={13} aria-hidden="true" />
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
      {startError && <p role="alert" className="text-xs text-critical">Could not start the chat: {startError}</p>}
      {error && issues && <p role="alert" className="text-xs text-critical">Refresh failed: {error}. Showing the last list.</p>}
    </div>
  );
}
