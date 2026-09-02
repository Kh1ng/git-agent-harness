import { useEffect, useState } from 'react';
import { GitBranch, GitCommit, GitPullRequest, RefreshCw, Plus, ExternalLink } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { gahApi } from '../api/client.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState, LoadingState, ErrorState } from '../components/ui/EmptyState.js';
import type { ChatPrSummary } from '@git-agent-harness/contracts';

interface GitStatus { branch: string; changes: { status: string; path: string }[]; cwd: string | null; readOnly?: boolean }
interface GitLog { commits: { hash: string; short: string; subject: string; author: string; ago: string }[] }
interface GitPrs { prs: ChatPrSummary[]; warning?: string }

type Tab = 'status' | 'log' | 'prs';

export function GitPage() {
  const wsProfile = useWebSocket().profile;
  const profileOverride = useUiStore((s) => s.profileOverride);
  const profile = profileOverride ?? wsProfile ?? 'gah';

  const [tab, setTab] = useState<Tab>('status');
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [log, setLog] = useState<GitLog | null>(null);
  const [prs, setPrs] = useState<GitPrs | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // PR creation form
  const [prTitle, setPrTitle] = useState('');
  const [prBody, setPrBody] = useState('');
  const [prDraft, setPrDraft] = useState(false);
  const [creatingPr, setCreatingPr] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const [showPrForm, setShowPrForm] = useState(false);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const [s, l, p] = await Promise.all([
        gahApi.getGitStatus(profile),
        gahApi.getGitLog(profile, 20),
        gahApi.getGitPrs(profile),
      ]);
      setStatus(s);
      setLog(l);
      setPrs(p);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, [profile]);

  const createPr = async () => {
    if (!prTitle) return;
    setCreatingPr(true);
    setCreateError(null);
    try {
      const result = await gahApi.createGitPr(profile, { title: prTitle, body: prBody, draft: prDraft });
      setShowPrForm(false);
      setPrTitle('');
      setPrBody('');
      // Refresh PRs
      const updated = await gahApi.getGitPrs(profile);
      setPrs(updated);
      window.open(result.url, '_blank');
    } catch (e) {
      setCreateError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreatingPr(false);
    }
  };

  const tabs: { id: Tab; label: string }[] = [
    { id: 'status', label: 'Status' },
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
          <div className="flex rounded-md border border-subtle overflow-hidden text-xs">
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
            <EmptyState icon={GitBranch} title="Working tree clean" description="Nothing to commit." />
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
            <button
              onClick={() => setShowPrForm((v) => !v)}
              className="btn-primary text-xs flex items-center gap-1.5"
            >
              <Plus size={12} />
              New PR
            </button>
          </div>

          {showPrForm && (
            <div className="card-padded space-y-3">
              <input
                type="text"
                value={prTitle}
                onChange={(e) => setPrTitle(e.target.value)}
                placeholder="PR title"
                className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary focus:outline-none focus:border-accent"
              />
              <textarea
                value={prBody}
                onChange={(e) => setPrBody(e.target.value)}
                placeholder="Description (optional)"
                rows={3}
                className="w-full bg-raised border border-subtle rounded-md px-3 py-2 text-sm text-primary resize-none focus:outline-none focus:border-accent"
              />
              <div className="flex items-center gap-4">
                <label className="flex items-center gap-2 text-xs text-secondary cursor-pointer">
                  <input type="checkbox" checked={prDraft} onChange={(e) => setPrDraft(e.target.checked)} className="accent-accent" />
                  Draft
                </label>
                <button
                  onClick={createPr}
                  disabled={!prTitle || creatingPr}
                  className="btn-primary text-xs px-3 py-1.5 disabled:opacity-50"
                >
                  {creatingPr ? 'Creating…' : 'Create PR'}
                </button>
                {createError && <p className="text-xs text-critical">{createError}</p>}
              </div>
            </div>
          )}

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
                    <tr key={i}>
                      <td className="text-muted text-xs">#{pr.number}</td>
                      <td className="text-sm text-primary">
                        {pr.title}
                        {pr.isDraft && <span className="ml-2 text-xs text-muted">[draft]</span>}
                      </td>
                      <td className="font-mono text-xs text-secondary">{pr.headRefName ?? ''}</td>
                      <td>
                        {pr.url ? (
                          <a href={pr.url} target="_blank" rel="noreferrer" className="text-muted hover:text-primary">
                            <ExternalLink size={13} />
                          </a>
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
    </div>
  );
}
