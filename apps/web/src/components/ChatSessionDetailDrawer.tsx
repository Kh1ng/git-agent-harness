import { useEffect, useRef, useState } from 'react';
import { Archive, History, RotateCcw, Save, X } from 'lucide-react';
import type { ChatSessionSummary, SkillBindingSummary } from '@git-agent-harness/contracts';
import { formatChatName } from '../lib/format.js';
import { ProviderPicker, type ProviderPickerProps } from './ProviderPicker.js';
import { StatusBadge } from './ui/StatusBadge.js';

export type SessionUsageSummary = {
  turns: number;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  estimatedCostUsd: number | null;
  costIncomplete: boolean;
};

type Props = {
  session: ChatSessionSummary;
  turnBusy: boolean;
  providerPicker: ProviderPickerProps | null;
  skillBinding: SkillBindingSummary | null;
  skillBusy: boolean;
  usage: SessionUsageSummary;
  workId: string | null;
  onRename: (title: string) => Promise<void>;
  onArchive: () => Promise<void>;
  onRestore: () => Promise<void>;
  onToggleSkill: (id: string) => Promise<void>;
  onInheritSkills: () => Promise<void>;
  onOpenWork?: (workId: string) => void;
  onClose: () => void;
};

function Metric({ label, value }: { label: string; value: string }) {
  return <div><dt className="text-xs text-muted">{label}</dt><dd className="mt-1 break-words text-sm text-primary">{value}</dd></div>;
}

function tokenCount(value: number | null): string {
  return value == null ? 'Not reported' : value.toLocaleString();
}

export function ChatSessionDetailDrawer({
  session, turnBusy, providerPicker, skillBinding, skillBusy, usage, workId,
  onRename, onArchive, onRestore, onToggleSkill, onInheritSkills, onOpenWork, onClose
}: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const [title, setTitle] = useState(session.title ?? '');
  const [busy, setBusy] = useState<'rename' | 'archive' | 'restore' | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    dialog.current?.showModal();
    closeButton.current?.focus();
    return () => dialog.current?.close();
  }, []);
  useEffect(() => setTitle(session.title ?? ''), [session.id, session.title]);

  const run = async (action: NonNullable<typeof busy>, callback: () => Promise<void>) => {
    setBusy(action);
    setError(null);
    try { await callback(); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(null); }
  };
  const archived = session.archivedAt !== null;
  const settled = session.outcome === 'settled';
  const controlsDisabled = turnBusy || busy !== null;
  const source = skillBinding?.source === 'session' ? 'Session override'
    : skillBinding?.source === 'profile' ? 'Project default' : 'Global default';

  return (
    <dialog ref={dialog} aria-labelledby="chat-session-detail-title" onCancel={onClose}
      onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}
      className="fixed inset-0 z-50 m-0 h-dvh max-h-none w-full max-w-none overflow-hidden border-0 bg-transparent p-0 backdrop:bg-scrim/35">
      <aside className="ml-auto flex h-full w-full min-w-0 flex-col overflow-hidden border-l border-subtle bg-page shadow-2xl sm:max-w-xl">
        <header className="flex shrink-0 items-start justify-between gap-4 border-b border-subtle px-4 py-4 sm:px-6">
          <div className="min-w-0">
            <p className="font-mono text-xs text-muted">{session.id}</p>
            <h2 id="chat-session-detail-title" className="mt-1 break-words text-lg font-semibold text-primary">{formatChatName(session)}</h2>
          </div>
          <button ref={closeButton} type="button" onClick={onClose} className="btn-secondary min-h-11 min-w-11 p-2" aria-label="Close session details">
            <X size={18} aria-hidden="true" />
          </button>
        </header>

        <div className="min-w-0 flex-1 space-y-6 overflow-y-auto overflow-x-hidden px-4 py-5 sm:px-6">
          <section aria-labelledby="session-identity-heading" className="space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h3 id="session-identity-heading" className="text-sm font-semibold text-primary">Session</h3>
              <StatusBadge tone={settled ? 'good' : archived ? 'unknown' : 'good'} label={settled ? `Settled · ${session.settledReason ?? 'delivered'}` : archived ? 'Archived' : 'Live'} />
            </div>
            <div className="flex gap-2">
              <input aria-label="Session name" value={title} onChange={(event) => setTitle(event.target.value)} disabled={archived || controlsDisabled}
                placeholder="Session name" className="min-w-0 flex-1 rounded-md border border-subtle bg-raised px-3 py-2 text-base text-primary sm:text-sm" />
              <button type="button" className="btn-secondary min-h-11" disabled={archived || controlsDisabled || !title.trim() || title.trim() === (session.title ?? '')}
                onClick={() => void run('rename', () => onRename(title.trim()))}>
                <Save size={14} aria-hidden="true" /> {busy === 'rename' ? 'Saving…' : 'Save'}
              </button>
            </div>
            <dl className="grid grid-cols-2 gap-3 rounded-lg border border-subtle bg-raised p-3 sm:grid-cols-3">
              <Metric label="Backend" value={session.backend} />
              <Metric label="Instance" value={session.backendInstance ?? 'Default'} />
              <Metric label="Model" value={session.model ?? 'Backend default'} />
              <Metric label="Reasoning" value={session.reasoningEffort ?? 'Backend default'} />
              <Metric label="Branch" value={session.branch || 'Read-only provider checkout'} />
              <Metric label="Workspace" value={session.remoteWorkspace ? 'Remote node' : session.worktreePath ? 'Worktree' : 'Read only'} />
            </dl>
            {!archived && providerPicker && <div><p className="mb-2 text-xs text-muted">Change provider, model, or reasoning</p><ProviderPicker {...providerPicker} triggerAriaLabel="Session provider and model" /></div>}
          </section>

          <section aria-labelledby="session-usage-heading">
            <h3 id="session-usage-heading" className="mb-3 text-sm font-semibold text-primary">Usage</h3>
            <dl className="grid grid-cols-2 gap-3 rounded-lg border border-subtle bg-raised p-3 sm:grid-cols-4">
              <Metric label="Reported turns" value={usage.turns.toLocaleString()} />
              <Metric label="Input tokens" value={tokenCount(usage.inputTokens)} />
              <Metric label="Output tokens" value={tokenCount(usage.outputTokens)} />
              <Metric label="Estimated cost" value={usage.estimatedCostUsd == null ? 'Not reported' : `$${usage.estimatedCostUsd.toFixed(4)}${usage.costIncomplete ? '+' : ''}`} />
            </dl>
            {usage.totalTokens != null && <p className="mt-2 text-xs text-muted">{usage.totalTokens.toLocaleString()} total reported tokens</p>}
          </section>

          <section aria-labelledby="session-skills-heading">
            <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
              <div><h3 id="session-skills-heading" className="text-sm font-semibold text-primary">Skills</h3><p className="text-xs text-muted">{skillBinding ? `${source} · ${skillBinding.backend}` : 'Loading binding…'}</p></div>
              {skillBinding?.source === 'session' && <button type="button" className="text-xs text-accent hover:underline disabled:opacity-50" disabled={archived || turnBusy || skillBusy} onClick={() => void onInheritSkills()}>Use project default</button>}
            </div>
            {!skillBinding ? <p className="text-xs text-muted">Loading available skills…</p>
              : !skillBinding.supported ? <p className="text-xs text-muted">This backend does not support bound skills.</p>
                : skillBinding.skills.length === 0 ? <p className="text-xs text-muted">No compatible skills are installed.</p>
                  : <div className="divide-y divide-subtle rounded-lg border border-subtle">
                    {skillBinding.skills.map((skill) => <label key={skill.id} className="flex cursor-pointer gap-3 px-3 py-2.5 hover:bg-overlay/5">
                      <input type="checkbox" className="mt-0.5 accent-[rgb(var(--accent))]" checked={skillBinding.selectedIds.includes(skill.id)}
                        disabled={archived || turnBusy || skillBusy} onChange={() => void onToggleSkill(skill.id)} />
                      <span className="min-w-0"><span className="block text-sm text-primary">{skill.displayName}</span><span className="block break-words text-xs text-muted">{skill.description || `${skill.id} · ${skill.version}`}</span></span>
                    </label>)}
                  </div>}
          </section>

          <section aria-labelledby="session-work-heading">
            <h3 id="session-work-heading" className="mb-3 text-sm font-semibold text-primary">Work association</h3>
            {workId ? <button type="button" className="btn-secondary min-h-11" onClick={() => { onOpenWork?.(workId); onClose(); }} disabled={!onOpenWork}>
              <History size={14} aria-hidden="true" /> Open {workId} and attempt history
            </button> : <p className="text-xs text-muted">This chat is not associated with a tracked work item.</p>}
          </section>

          <section aria-labelledby="session-lifecycle-heading" className="border-t border-subtle pt-5">
            <h3 id="session-lifecycle-heading" className="mb-3 text-sm font-semibold text-primary">Lifecycle</h3>
            {!archived ? <button type="button" className="btn-secondary min-h-11 text-critical border-critical/30" disabled={controlsDisabled}
              onClick={() => { if (window.confirm('Archive this session? Dirty work is saved as a patch and the branch survives.')) void run('archive', onArchive); }}>
              <Archive size={14} aria-hidden="true" /> {busy === 'archive' ? 'Archiving…' : 'Archive session'}
            </button> : !settled ? <button type="button" className="btn-primary min-h-11" disabled={controlsDisabled} onClick={() => void run('restore', onRestore)}>
              <RotateCcw size={14} aria-hidden="true" /> {busy === 'restore' ? 'Restoring…' : 'Restore session'}
            </button> : <p className="text-xs text-muted">Settled sessions are terminal and cannot be restored.</p>}
            {turnBusy && <p className="mt-2 text-xs text-warning">Stop the active turn before changing session settings or lifecycle.</p>}
            {error && <p role="alert" className="mt-2 text-xs text-critical">{error}</p>}
          </section>
        </div>
      </aside>
    </dialog>
  );
}
