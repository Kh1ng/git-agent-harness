import { useEffect, useState } from 'react';
import { FolderGit2, GitBranch, GitCommit, GitPullRequest, ExternalLink } from 'lucide-react';
import { ExternalAnchor } from '../components/ExternalAnchor';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { gahApi, type GitWorktreeSummary } from '../api/client.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import { CommitPrDialog } from '../components/CommitPrDialog.js';
import { OpenLocalCheckout } from '../components/OpenLocalCheckout.js';
import type { ChatPrSummary } from '@git-agent-harness/contracts';

interface GitStatus { branch: string; changes: { status: string; path: string }[]; cwd: string | null; readOnly?: boolean; ownerNodeId: string; ownerNodeName: string }
interface GitLog { commits: { hash: string; short: string; subject: string; author: string; ago: string }[] }
interface GitPrs { prs: ChatPrSummary[]; warning?: string }

type Tab = 'status' | 'worktrees' | 'log' | 'prs';

export function GitPage() {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const setProfileOverride = useUiStore((s) => s.setProfileOverride);
  const profile = profileOverride ?? wsProfile ?? 'gah';
  const profiles = useGahStore((s) => s.profiles);
  const dashboardStatus = useGahStore((s) => s.status.data);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);

  const [tab, setTab] = useState<Tab>('status');
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [log, setLog] = useState<GitLog | null>(null);
  const [prs, setPrs] = useState<GitPrs | null>(null);
  const [worktrees, setWorktrees] = useState<GitWorktreeSummary[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [reviewOpen, setReviewOpen] = useState(false);
  const [reviewTarget, setReviewTarget] = useState<ChatPrSummary | null>(null);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const [s, l, p, w] = await Promise.all([
        gahApi.getGitStatus(profile),
        gahApi.getGitLog(profile, 20),
        gahApi.getGitPrs(profile),
        // An older server has no worktree route; the rest of the page still loads.
        gahApi.getGitWorktrees(profile).catch(() => null),
      ]);
      setStatus(s);
      setLog(l);
      setPrs(p);
      setWorktrees(w ? w.worktrees.filter((worktree) => !worktree.main) : null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, [profile]);
  useEffect(() => { fetchProfiles(); }, [fetchProfiles]);
  useWsReconnectRefresh(() => { void load(); void fetchProfiles({ force: true }); });

  const tabs: { id: Tab; label: string }[] = [
    { id: 'status', label: 'Status' },
    { id: 'worktrees', label: worktrees?.length ? `Worktrees (${worktrees.length})` : 'Worktrees' },
    { id: 'log', label: 'Log' },
    { id: 'prs', label: 'Pull Requests' },
  ];

  return (
    <div className="space-y-6">
      <PageHeader
        title="Git"
        description={status ? `${status.branch} · ${status.cwd}` : `Profile: ${profile}`}
        onRefresh={load}
        refreshing={loading}
        actions={
          <div className="flex flex-wrap items-center justify-end gap-2">
            <select
              aria-label="Project"
              value={profile}
              onChange={(event) => setProfileOverride(event.target.value)}
              disabled={profiles.loading && !profiles.data}
              className="rounded-md border border-subtle bg-raised px-3 py-1.5 text-xs text-primary"
            >
              {(profiles.data ?? []).map((candidate) => (
                <option key={candidate.name} value={candidate.name}>
                  {candidate.display_name || candidate.name}
                </option>
              ))}
            </select>
            {status && <OpenLocalCheckout profile={profile} nodeId={status.ownerNodeId} nodeName={status.ownerNodeName} />}
            <button type="button" className="btn-primary text-xs" onClick={() => { setReviewTarget(null); setReviewOpen(true); }}>Commit / PR</button>
            <div className="flex overflow-hidden rounded-md border border-subtle text-xs">
              {tabs.map((t) => (
                <button
                  key={t.id}
                  onClick={() => setTab(t.id)}
                  className={`px-3 py-1.5 ${tab === t.id ? 'bg-accent text-white' : 'text-secondary hover:bg-white/5'}`}
                >
                  {t.label}
                </button>
              ))}
            </div>
          </div>
        }
      />

      {loading && !status && <LoadingState label="Loading git data…" />}
      {error && <ErrorState message={error} endpoint="/api/git/*" onRetry={load} />}

      {tab === 'status' && status && (
        <div className="space-y-4">
          <div className="card-padded flex items-center gap-3">
            <GitBranch size={16} className="text-accent" />
            <span className="text-sm font-mono text-primary">{status.branch}</span>
            <span className="text-xs text-muted">{status.changes.length} changed file{status.changes.length !== 1 ? 's' : ''}</span>
          </div>
          {status.changes.length === 0 ? (
            <EmptyState
              icon={GitBranch}
              title="Working tree clean"
              description={worktrees?.length
                ? `Nothing to commit in this checkout. Agents are working in ${worktrees.length} separate worktree${worktrees.length !== 1 ? 's' : ''}; see the Worktrees tab.`
                : 'Nothing to commit.'}
            />
          ) : (
            <div className="card overflow-hidden">
              <table className="table-base">
                <thead>
                  <tr>
                    <th className="w-16">Status</th>
                    <th>Path</th>
                  </tr>
                </thead>
                <tbody>
                  {status.changes.map((c, i) => (
                    <tr key={i}>
                      <td><span className="font-mono text-xs bg-raised px-1.5 py-0.5 rounded">{c.status}</span></td>
                      <td className="font-mono text-xs text-primary">{c.path}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}

      {tab === 'worktrees' && !worktrees && !loading && (
        <EmptyState icon={FolderGit2} title="Worktrees unavailable" description="This server does not report worktrees yet. Update it to see them here." />
      )}
      {tab === 'worktrees' && worktrees && (
        <div className="space-y-4">
          <p className="text-xs text-muted">
            Each agent job and chat works in its own copy of the repository. A clean worktree is removed automatically once its pull request is merged or closed.
          </p>
          {worktrees.length === 0 ? (
            <EmptyState icon={FolderGit2} title="No worktrees" description="No agent job or chat has a worktree right now." />
          ) : (
            <div className="card overflow-hidden">
              <table className="table-base">
                <thead>
                  <tr>
                    <th>Branch</th>
                    <th>Pull request</th>
                    <th className="w-28">Changed files</th>
                    <th>Path</th>
                  </tr>
                </thead>
                <tbody>
                  {worktrees.map((worktree) => (
                    <tr key={worktree.path}>
                      <td className="font-mono text-xs text-primary">{worktree.branch ?? `detached at ${worktree.head.slice(0, 8)}`}</td>
                      <td className="text-sm text-primary">
                        {worktree.pullRequest ? (
                          <span className="inline-flex items-center gap-2">
                            <span className="text-xs text-muted">#{worktree.pullRequest.number}</span>
                            {worktree.pullRequest.title}
                            {worktree.pullRequest.isDraft && <span className="text-xs text-muted">[draft]</span>}
                            {worktree.pullRequest.url && (
                              <ExternalAnchor href={worktree.pullRequest.url} className="text-muted hover:text-primary">
                                <ExternalLink size={13} />
                              </ExternalAnchor>
                            )}
                          </span>
                        ) : (
                          <span className="text-xs text-muted">No open pull request</span>
                        )}
                      </td>
                      <td className="text-xs text-secondary">{worktree.changedFiles === null ? 'unreadable' : worktree.changedFiles}</td>
                      <td className="font-mono text-xs text-muted">{worktree.path}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}

      {tab === 'log' && log && (
        <div className="card overflow-hidden">
          <table className="table-base">
            <thead>
              <tr>
                <th className="w-16">Hash</th>
                <th>Subject</th>
                <th className="w-32">Author</th>
                <th className="w-24">When</th>
              </tr>
            </thead>
            <tbody>
              {log.commits.map((c) => (
                <tr key={c.hash}>
                  <td><span className="font-mono text-xs text-muted">{c.short}</span></td>
                  <td className="text-sm text-primary">{c.subject}</td>
                  <td className="text-xs text-secondary">{c.author}</td>
                  <td className="text-xs text-muted">{c.ago}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {log.commits.length === 0 && (
            <EmptyState icon={GitCommit} title="No commits" description="No commit history found." />
          )}
        </div>
      )}

      {tab === 'prs' && prs && (
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-semibold text-primary">Open pull requests</h3>
            <button type="button" onClick={() => { setReviewTarget(null); setReviewOpen(true); }} className="btn-primary text-xs">Review and create</button>
          </div>

          {prs.warning && <p className="text-xs text-muted italic">{prs.warning}</p>}

          {prs.prs.length === 0 && !prs.warning ? (
            <EmptyState icon={GitPullRequest} title="No open PRs" description="No open pull requests for this profile." />
          ) : (
            <div className="card overflow-hidden">
              <table className="table-base">
                <thead>
                  <tr>
                    <th className="w-16">#</th>
                    <th>Title</th>
                    <th className="w-24">Branch</th>
                    <th className="w-16"></th>
                  </tr>
                </thead>
                <tbody>
                  {prs.prs.map((pr, i) => (
                    <tr key={i} className="cursor-pointer hover:bg-raised/50" onClick={() => { setReviewTarget(pr); setReviewOpen(true); }}>
                      <td className="text-muted text-xs">#{pr.number}</td>
                      <td className="text-sm text-primary">
                        {pr.title}
                        {pr.isDraft && <span className="ml-2 text-xs text-muted">[draft]</span>}
                      </td>
                      <td className="font-mono text-xs text-secondary">{pr.headRefName ?? ''}</td>
                      <td>
                        {pr.url ? (
                          <ExternalAnchor href={pr.url} onClick={(event) => event.stopPropagation()} className="text-muted hover:text-primary">
                            <ExternalLink size={13} />
                          </ExternalAnchor>
                        ) : null}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
      {reviewOpen && (
        <CommitPrDialog
          profile={profile}
          nodeId={status?.ownerNodeId}
          providerRequest={reviewTarget}
          mergeRequest={reviewTarget ? dashboardStatus?.merge_requests.find(request => request.branch === reviewTarget.headRefName) : null}
          onClose={() => setReviewOpen(false)}
          onChanged={() => void load()}
        />
      )}
    </div>
  );
}
