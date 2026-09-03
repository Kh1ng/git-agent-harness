import { useMemo, useState } from 'react';
import { ChevronRight, FolderGit2 } from 'lucide-react';
import type { ChatSessionSummary, ProfileSummary } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';

/**
 * The chat page's navigator: every configured project and every conversation
 * in the selected project is one click away. Archived conversations stay
 * available behind a disclosure instead of crowding the working set.
 *
 * The curated catalog (`/api/projects`) is no longer the gate it once was:
 * it drives the Overview dashboard, while chat lists all configured
 * profiles directly, so an un-curated profile is still chattable.
 */
export function ProjectRail({
  currentProfile,
  profiles,
  sessions,
  selectedSessionId,
  onSelect,
  onSessionSelect,
  sessionsError,
  onRetrySessions,
  onProjectAdded
}: {
  currentProfile: string;
  profiles: ProfileSummary[];
  sessions: ChatSessionSummary[];
  selectedSessionId: string | null;
  onSelect: (profile: string | null) => void;
  onSessionSelect: (sessionId: string | null) => void;
  sessionsError: boolean;
  onRetrySessions: () => void;
  onProjectAdded: (profile: ProfileSummary) => void;
}) {
  const [gitUrl, setGitUrl] = useState('');
  const [reclone, setReclone] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ tone: 'error' | 'success'; text: string } | null>(null);

  const sorted = useMemo(
    () => [...profiles].sort((a, b) => (a.display_name || a.name).localeCompare(b.display_name || b.name)),
    [profiles]
  );
  const liveSessions = sessions.filter((session) => session.outcome === 'live');
  const archivedSessions = sessions.filter((session) => session.outcome !== 'live');
  const sessionClasses = (active: boolean) =>
    `block w-full rounded-md px-2 py-1.5 text-left ${active ? 'bg-accent/15' : 'hover:bg-white/5'}`;

  const importProject = async () => {
    if (!gitUrl.trim()) return;
    setSaving(true);
    setMessage(null);
    try {
      const result = await gahApi.importProject({ gitUrl: gitUrl.trim(), reclone });
      onProjectAdded(result.project);
      onSelect(result.project.name);
      setGitUrl('');
      setReclone(false);
      const languages = result.detectedLanguages.length > 0
        ? ` Detected ${result.detectedLanguages.join(', ')}.`
        : '';
      const validation = result.validationCommands.length > 0
        ? ` Validation: ${result.validationCommands.join(', ')}.`
        : ' No validation command was detected.';
      setMessage({ tone: 'success', text: `${result.checkoutStatus} ${result.project.repo}.${languages}${validation}` });
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <aside className="card p-3 xl:h-[65vh] xl:overflow-y-auto" aria-label="Chat navigation">
      <details open>
        <summary className="group flex cursor-pointer list-none items-center justify-between gap-2 rounded px-1 pb-2 text-xs font-semibold uppercase tracking-wide text-muted marker:content-none hover:text-primary">
          <span className="flex items-center gap-1">
            <ChevronRight size={12} className="transition-transform group-open:rotate-90" aria-hidden="true" />
            Projects
          </span>
          <span className="tabular-nums">{sorted.length}</span>
        </summary>

        <nav aria-label="Projects" className="space-y-1">
          {sorted.length === 0 && (
            <p className="px-2 py-3 text-xs leading-relaxed text-muted">Import a repository below to start.</p>
          )}
          {sorted.map((project) => {
            const active = project.name === currentProfile;
            return (
              <button
                key={project.name}
                type="button"
                onClick={() => onSelect(project.name)}
                className={`block w-full rounded-md px-2 py-2 text-left ${active ? 'bg-accent/15' : 'hover:bg-white/5'}`}
                aria-current={active ? 'page' : undefined}
              >
                <span className="block truncate text-sm font-medium text-primary">{project.display_name || project.name}</span>
                <span className="block truncate text-[11px] text-muted">{project.repo}</span>
              </button>
            );
          })}
        </nav>
      </details>

      <section className="mt-4 border-t border-subtle pt-3" aria-labelledby="chat-list-title">
        <div className="flex items-center justify-between gap-2 px-1 pb-2">
          <h3 id="chat-list-title" className="text-xs font-semibold uppercase tracking-wide text-muted">Chats</h3>
          {sessionsError ? (
            <button type="button" onClick={onRetrySessions} className="text-[11px] text-amber-300 hover:text-primary">Retry</button>
          ) : (
            <span className="text-xs tabular-nums text-muted">{liveSessions.length + 1}</span>
          )}
        </div>
        <nav aria-label="Chats" className="space-y-1">
          <button
            type="button"
            onClick={() => onSessionSelect(null)}
            className={sessionClasses(selectedSessionId === null)}
            aria-current={selectedSessionId === null ? 'page' : undefined}
          >
            <span className="block truncate text-sm text-primary">Default conversation</span>
          </button>
          {liveSessions.map((session) => (
            <button
              key={session.id}
              type="button"
              onClick={() => onSessionSelect(session.id)}
              className={sessionClasses(session.id === selectedSessionId)}
              aria-current={session.id === selectedSessionId ? 'page' : undefined}
            >
              <span className="block truncate text-sm text-primary">{session.title ?? session.branch}</span>
              <span className="block truncate text-[11px] text-muted">{session.branch}</span>
            </button>
          ))}
        </nav>

        {archivedSessions.length > 0 && (
          <details className="mt-2">
            <summary className="cursor-pointer select-none rounded px-2 py-1 text-xs text-muted hover:bg-white/5 hover:text-primary">
              Archived ({archivedSessions.length})
            </summary>
            <nav aria-label="Archived chats" className="mt-1 space-y-1">
              {archivedSessions.map((session) => (
                <button
                  key={session.id}
                  type="button"
                  onClick={() => onSessionSelect(session.id)}
                  className={sessionClasses(session.id === selectedSessionId)}
                  aria-current={session.id === selectedSessionId ? 'page' : undefined}
                >
                  <span className="block truncate text-sm text-secondary">{session.title ?? session.branch}</span>
                  <span className="block truncate text-[11px] text-muted">{session.branch}</span>
                </button>
              ))}
            </nav>
          </details>
        )}
      </section>

      <div className="mt-4 border-t border-subtle pt-3 space-y-2">
        <details className="pt-1">
          <summary className="cursor-pointer select-none text-xs font-medium text-secondary hover:text-primary">Import from Git</summary>
          <div className="mt-2 space-y-2">
            <label className="sr-only" htmlFor="project-git-url">Git repository URL</label>
            <input
              id="project-git-url"
              type="url"
              value={gitUrl}
              onChange={(event) => setGitUrl(event.target.value)}
              placeholder="https://github.com/owner/repo"
              className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-xs text-primary placeholder:text-muted"
            />
            <label className="flex items-start gap-2 text-[11px] leading-snug text-muted">
              <input type="checkbox" checked={reclone} onChange={(event) => setReclone(event.target.checked)} className="mt-0.5" />
              Re-clone an existing clean managed checkout
            </label>
            <button type="button" onClick={importProject} disabled={saving || !gitUrl.trim()} className="btn-primary w-full !min-h-0 text-xs">
              <FolderGit2 size={14} aria-hidden="true" />
              {saving ? 'Working…' : 'Import repository'}
            </button>
          </div>
        </details>

        {message && (
          <p role={message.tone === 'error' ? 'alert' : 'status'} className={`text-[11px] leading-relaxed ${message.tone === 'error' ? 'text-red-400' : 'text-secondary'}`}>
            {message.text}
          </p>
        )}
      </div>
    </aside>
  );
}
