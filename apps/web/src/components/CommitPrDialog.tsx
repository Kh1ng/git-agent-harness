import { useEffect, useRef, useState } from 'react';
import { ExternalLink, GitCommit, GitPullRequest, RefreshCw, X } from 'lucide-react';
import type { GitReviewState, HelperSuggestion } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';

interface CommitPrDialogProps {
  profile: string;
  sessionId?: string;
  nodeId?: string;
  onClose: () => void;
  onChanged: () => void;
}

function suggestedBody(review: GitReviewState): string {
  const commits = review.commits.map(commit => `- ${commit.subject}`).join('\n');
  const files = review.changedFiles.map(path => `- \`${path}\``).join('\n');
  return `${commits ? `## Commits\n\n${commits}\n\n` : ''}## Changed files\n\n${files || '- None'}`;
}

/** One review-first commit and publish flow shared by Chat and Git. The server
 * resolves every path from profile/session identity on the owning node. */
export function CommitPrDialog({ profile, sessionId, nodeId, onClose, onChanged }: CommitPrDialogProps) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [review, setReview] = useState<GitReviewState | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [base, setBase] = useState('');
  const [message, setMessage] = useState('');
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [draft, setDraft] = useState(false);
  const [continueDirty, setContinueDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [published, setPublished] = useState<string | null>(null);
  const [commitSuggestion, setCommitSuggestion] = useState<HelperSuggestion | null>(null);
  const [prSuggestion, setPrSuggestion] = useState<HelperSuggestion | null>(null);
  const [suggesting, setSuggesting] = useState<'commit' | 'pr' | null>(null);
  const messageEdited = useRef(false);
  const titleEdited = useRef(false);
  const bodyEdited = useRef(false);
  const messageRevision = useRef(0);
  const titleRevision = useRef(0);
  const bodyRevision = useRef(0);

  const load = async (requestedBase?: string) => {
    setBusy(true);
    setError(null);
    try {
      const next = await gahApi.getGitReview(profile, { sessionId, nodeId, base: requestedBase || undefined });
      messageRevision.current++;
      titleRevision.current++;
      bodyRevision.current++;
      setCommitSuggestion(null);
      setPrSuggestion(null);
      setReview(next);
      setBase(next.base);
      setSelected(new Set(next.files.map(file => file.path)));
      setContinueDirty(next.files.length === 0);
      if (!titleEdited.current) setTitle(next.existing?.title ?? next.commits[0]?.subject ?? '');
      if (!bodyEdited.current) setBody(suggestedBody(next));
      setDraft(next.existing?.draft ?? false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    dialog.current?.showModal();
    void load();
  }, [profile, sessionId, nodeId]);

  const commit = async (files?: string[]) => {
    if (!message.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await gahApi.createGitCommit(profile, message.trim(), sessionId, files, nodeId);
      setMessage('');
      setCommitSuggestion(null);
      messageEdited.current = false;
      await load(base);
      onChanged();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      setBusy(false);
    }
  };

  const suggestCommit = async () => {
    if (selected.size === 0 || busy) return;
    if (messageEdited.current && !window.confirm('Replace your edited commit message with a model suggestion?')) return;
    const revision = messageRevision.current;
    setBusy(true);
    setSuggesting('commit');
    setError(null);
    try {
      const suggestion = await gahApi.suggestGitProse(profile, { kind: 'commit_message', sessionId, nodeId, files: selectedFiles });
      if (messageRevision.current !== revision) return;
      setCommitSuggestion(suggestion);
      if (suggestion.generated) {
        setMessage(suggestion.text);
        messageEdited.current = false;
      } else {
        setError(`No model suggestion was available (${suggestion.fallbackReason ?? 'unknown reason'}). Enter a commit message manually.`);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
      setSuggesting(null);
    }
  };

  const applyPrSuggestion = async () => {
    if (!review) return;
    if ((titleEdited.current || bodyEdited.current) && !window.confirm('Replace your edited title and body with a model suggestion?')) return;
    const titleAtRequest = titleRevision.current;
    const bodyAtRequest = bodyRevision.current;
    setBusy(true);
    setSuggesting('pr');
    setError(null);
    try {
      const suggestion = await gahApi.suggestGitProse(profile, { kind: 'pr_summary', sessionId, nodeId, base });
      if (titleRevision.current !== titleAtRequest || bodyRevision.current !== bodyAtRequest) return;
      setPrSuggestion(suggestion);
      if (suggestion.generated) {
        setTitle(suggestion.title ?? '');
        setBody(suggestion.body ?? '');
        titleEdited.current = false;
        bodyEdited.current = false;
      } else {
        setError(`No model suggestion was available (${suggestion.fallbackReason ?? 'unknown reason'}). Enter the title and body manually.`);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
      setSuggesting(null);
    }
  };

  const publish = async () => {
    if (!review || !title.trim() || busy || base.trim() !== review.base) return;
    setBusy(true);
    setError(null);
    try {
      const result = await gahApi.publishGitPr(profile, { title: title.trim(), body, base: base.trim(), draft, sessionId, nodeId });
      setPublished(result.url);
      setPrSuggestion(null);
      onChanged();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const selectedFiles = [...selected];
  const dirty = (review?.files.length ?? 0) > 0;
  const canReviewPr = !!review && review.commits.length > 0 && (!dirty || continueDirty);
  const baseNeedsReview = !!review && base.trim() !== review.base;

  return (
    <dialog ref={dialog} onClose={onClose} aria-label="Commit and pull request review"
      className="card m-auto max-h-[92dvh] w-[min(58rem,calc(100vw-2rem))] overflow-y-auto p-0 text-primary backdrop:bg-black/70">
      <header className="sticky top-0 z-10 flex items-start justify-between gap-4 border-b border-subtle bg-card px-5 py-4">
        <div>
          <h2 className="text-base font-semibold">Commit and {review?.providerLabel ?? 'pull request'}</h2>
          <p className="mt-1 text-xs text-muted">Review first. Nothing reaches the remote until the final push action.</p>
        </div>
        <button type="button" onClick={() => dialog.current?.close()} aria-label="Close commit review" className="text-muted hover:text-primary"><X size={18} /></button>
      </header>

      <div className="space-y-5 p-5">
        {busy && !review && <p className="text-sm text-muted">Preparing review…</p>}
        {error && <p role="alert" className="rounded-md border border-critical/40 bg-critical/10 px-3 py-2 text-sm text-critical">{error}</p>}
        <span className="sr-only" aria-live="polite">{suggesting === 'commit' ? 'Generating commit message suggestion' : suggesting === 'pr' ? 'Generating pull request suggestion' : ''}</span>
        {review && (
          <>
            <section className="grid gap-2 rounded-md border border-subtle bg-raised p-3 text-xs sm:grid-cols-2">
              <p><span className="text-muted">Checkout owner:</span> {review.ownerNodeName}</p>
              <p><span className="text-muted">Branch:</span> <code>{review.branch}</code></p>
              <p><span className="text-muted">Upstream:</span> {review.upstream ?? 'not pushed'}</p>
              <p><span className="text-muted">Upstream state:</span> {review.ahead} ahead, {review.behind} behind</p>
            </section>

            {dirty && (
              <section className="space-y-3">
                <div className="flex items-center justify-between gap-3">
                  <h3 className="text-sm font-semibold">Local changes</h3>
                  <span className="text-xs text-muted">{selected.size} of {review.files.length} selected</span>
                </div>
                <div className="max-h-48 overflow-y-auto rounded-md border border-subtle">
                  {review.files.map(file => (
                    <label key={file.path} className="flex cursor-pointer items-center gap-3 border-b border-subtle px-3 py-2 text-xs last:border-b-0">
                      <input type="checkbox" checked={selected.has(file.path)} onChange={event => {
                        const next = new Set(selected);
                        if (event.target.checked) next.add(file.path); else next.delete(file.path);
                        messageRevision.current++;
                        setCommitSuggestion(null);
                        setSelected(next);
                      }} />
                      <code className="min-w-0 flex-1 break-all">{file.path}</code>
                      <span className="text-muted">{[file.staged && 'staged', file.unstaged && 'unstaged', file.untracked && 'untracked'].filter(Boolean).join(', ')}</span>
                    </label>
                  ))}
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <input aria-label="Commit message" value={message} onChange={event => { messageEdited.current = true; messageRevision.current++; setCommitSuggestion(null); setMessage(event.target.value); }} placeholder="Commit message"
                    className="min-w-52 flex-1 rounded-md border border-subtle bg-raised px-3 py-2 text-sm focus:border-accent" />
                  <button type="button" className="btn-secondary text-xs" disabled={busy || selected.size === 0} onClick={() => void suggestCommit()}>{suggesting === 'commit' ? 'Suggesting…' : commitSuggestion ? 'Regenerate' : 'Suggest'}</button>
                  <button type="button" className="btn-primary text-xs" disabled={busy || !message.trim() || selected.size === 0}
                    onClick={() => void commit(selectedFiles)}><GitCommit size={13} /> Commit selected</button>
                  <button type="button" className="btn-secondary text-xs" disabled={busy || !message.trim()}
                    onClick={() => void commit()}>Commit all</button>
                </div>
                {commitSuggestion?.generated && <p className="text-xs text-muted">Suggested by {commitSuggestion.backendInstance ?? commitSuggestion.backend} · {commitSuggestion.actualModel ?? commitSuggestion.effectiveModel}</p>}
                {!!commitSuggestion?.skippedFiles?.length && <p className="text-xs text-warning">Not sent to the helper: {commitSuggestion.skippedFiles.join(', ')}</p>}
                <p className="text-xs text-muted">Unselected changes stay local and will not enter this {review.providerLabel}.</p>
                {!continueDirty && review.commits.length > 0 && (
                  <button type="button" className="btn-secondary text-xs" onClick={() => setContinueDirty(true)}>Continue with uncommitted changes</button>
                )}
              </section>
            )}

            {canReviewPr && (
              <section className="space-y-4 border-t border-subtle pt-5">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <h3 className="text-sm font-semibold">Final {review.providerLabel} review</h3>
                  <button type="button" className="btn-secondary text-xs" disabled={busy} onClick={() => void applyPrSuggestion()}>{suggesting === 'pr' ? 'Suggesting…' : prSuggestion ? 'Regenerate title and body' : 'Suggest title and body'}</button>
                </div>
                <div className="grid gap-3 sm:grid-cols-2">
                  <label className="space-y-1 text-xs text-secondary">Base branch
                    <span className="flex gap-2"><input aria-label="Base branch" value={base} onChange={event => {
                      titleRevision.current++;
                      bodyRevision.current++;
                      setPrSuggestion(null);
                      setBase(event.target.value);
                    }}
                      className="min-w-0 flex-1 rounded-md border border-subtle bg-raised px-3 py-2 text-primary focus:border-accent" />
                    <button type="button" className="btn-secondary" disabled={busy || !base.trim()} onClick={() => void load(base)} aria-label="Refresh base review"><RefreshCw size={13} /></button></span>
                  </label>
                  <label className="space-y-1 text-xs text-secondary">Head branch
                    <input value={review.branch} readOnly className="w-full rounded-md border border-subtle bg-raised px-3 py-2 text-muted" />
                  </label>
                </div>
                <label className="block space-y-1 text-xs text-secondary">Title
                  <input aria-label="Pull request title" value={title} onChange={event => { titleEdited.current = true; titleRevision.current++; setPrSuggestion(null); setTitle(event.target.value); }}
                    className="w-full rounded-md border border-subtle bg-raised px-3 py-2 text-sm text-primary focus:border-accent" />
                </label>
                <label className="block space-y-1 text-xs text-secondary">Body
                  <textarea aria-label="Pull request body" rows={7} value={body} onChange={event => { bodyEdited.current = true; bodyRevision.current++; setPrSuggestion(null); setBody(event.target.value); }}
                    className="w-full resize-y rounded-md border border-subtle bg-raised px-3 py-2 text-sm text-primary focus:border-accent" />
                </label>
                {prSuggestion?.generated && <p className="text-xs text-muted">Suggested by {prSuggestion.backendInstance ?? prSuggestion.backend} · {prSuggestion.actualModel ?? prSuggestion.effectiveModel}</p>}
                {!!prSuggestion?.skippedFiles?.length && <p className="text-xs text-warning">Not sent to the helper: {prSuggestion.skippedFiles.join(', ')}</p>}
                <label className="flex items-center gap-2 text-xs text-secondary"><input type="checkbox" checked={draft} onChange={event => setDraft(event.target.checked)} /> Draft</label>
                {baseNeedsReview && <p role="status" className="text-xs text-warning">Refresh the review before publishing to a different base branch.</p>}

                <div className="grid gap-3 text-xs sm:grid-cols-3">
                  <div className="rounded-md border border-subtle p-3"><p className="mb-2 text-muted">Commits</p>{review.commits.map(commit => <p key={commit.hash}><code>{commit.short}</code> {commit.subject}</p>)}</div>
                  <div className="rounded-md border border-subtle p-3"><p className="mb-2 text-muted">Committed files</p>{review.changedFiles.map(path => <p key={path} className="break-all">{path}</p>)}</div>
                  <div className="rounded-md border border-subtle p-3"><p className="mb-2 text-muted">Excluded local files</p>{review.files.length ? review.files.map(file => <p key={file.path} className="break-all">{file.path}</p>) : <p>None</p>}</div>
                </div>
                <details className="rounded-md border border-subtle p-3"><summary className="cursor-pointer text-xs font-medium">Committed diff</summary><pre className="mt-3 max-h-72 overflow-auto whitespace-pre-wrap text-[11px] text-secondary">{review.patch || 'No committed diff.'}</pre></details>
                <div className="flex flex-wrap items-center gap-3">
                  <button type="button" className="btn-primary text-xs" disabled={busy || !title.trim() || baseNeedsReview} onClick={() => void publish()}>
                    <GitPullRequest size={13} /> Push and {review.existing ? 'update' : 'create'} {draft ? `draft ${review.providerLabel}` : review.providerLabel}
                  </button>
                  {published && <a href={published} target="_blank" rel="noreferrer" className="flex items-center gap-1 text-xs text-accent hover:underline"><ExternalLink size={13} /> Open {review.providerLabel}</a>}
                </div>
              </section>
            )}
          </>
        )}
      </div>
    </dialog>
  );
}
