import { useEffect, useMemo, useState } from 'react';
import { FolderGit2, Plus, Trash2 } from 'lucide-react';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';

export function ProjectRail({
  currentProfile,
  profiles,
  onSelect,
  onProjectAdded
}: {
  currentProfile: string;
  profiles: ProfileSummary[];
  onSelect: (profile: string | null) => void;
  onProjectAdded: (profile: ProfileSummary) => void;
}) {
  const [projects, setProjects] = useState<ProfileSummary[]>([]);
  const [profileToAdd, setProfileToAdd] = useState('');
  const [gitUrl, setGitUrl] = useState('');
  const [reclone, setReclone] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ tone: 'error' | 'success'; text: string } | null>(null);

  useEffect(() => {
    let cancelled = false;
    gahApi.getProjects()
      .then((items) => { if (!cancelled) setProjects(items); })
      .catch((error) => {
        if (!cancelled) setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
      })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);

  const available = useMemo(
    () => profiles.filter((profile) => !projects.some((project) => project.name === profile.name)),
    [profiles, projects]
  );

  useEffect(() => {
    if (!available.some((profile) => profile.name === profileToAdd)) {
      setProfileToAdd(available[0]?.name ?? '');
    }
  }, [available, profileToAdd]);

  const addExisting = async () => {
    if (!profileToAdd) return;
    setSaving(true);
    setMessage(null);
    try {
      const project = await gahApi.addProject(profileToAdd);
      setProjects((items) => [...items, project]);
      onSelect(project.name);
      setMessage({ tone: 'success', text: `${project.display_name || project.name} added.` });
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    } finally {
      setSaving(false);
    }
  };

  const importProject = async () => {
    if (!gitUrl.trim()) return;
    setSaving(true);
    setMessage(null);
    try {
      const result = await gahApi.importProject({ gitUrl: gitUrl.trim(), reclone });
      setProjects((items) => [...items.filter((item) => item.name !== result.project.name), result.project]);
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

  const remove = async (profile: string) => {
    setSaving(true);
    setMessage(null);
    try {
      await gahApi.removeProject(profile);
      const remaining = projects.filter((project) => project.name !== profile);
      setProjects(remaining);
      if (currentProfile === profile) onSelect(remaining[0]?.name ?? null);
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <aside className="card p-3 xl:h-[65vh] xl:overflow-y-auto" aria-label="Project rail">
      <div className="flex items-center justify-between gap-2 px-1 pb-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted">Projects</h3>
        {!loading && <span className="text-xs tabular-nums text-muted">{projects.length}</span>}
      </div>

      <nav aria-label="Projects" className="space-y-1">
        {loading && <p className="px-2 py-3 text-xs text-muted">Loading projects…</p>}
        {!loading && projects.length === 0 && (
          <p className="px-2 py-3 text-xs leading-relaxed text-muted">Add a configured profile or import a repository to start.</p>
        )}
        {projects.map((project) => {
          const active = project.name === currentProfile;
          return (
            <div key={project.name} className={`group flex items-center rounded-md ${active ? 'bg-accent/15' : 'hover:bg-white/5'}`}>
              <button
                type="button"
                onClick={() => onSelect(project.name)}
                className="min-w-0 flex-1 px-2 py-2 text-left focus-visible:rounded-md"
                aria-current={active ? 'page' : undefined}
              >
                <span className="block truncate text-sm font-medium text-primary">{project.display_name || project.name}</span>
                <span className="block truncate text-[11px] text-muted">{project.repo}</span>
              </button>
              <button
                type="button"
                onClick={() => remove(project.name)}
                disabled={saving}
                className="mr-1 rounded p-1.5 text-muted opacity-60 hover:bg-raised hover:text-primary group-hover:opacity-100 focus-visible:opacity-100 disabled:opacity-30"
                aria-label={`Remove ${project.display_name || project.name} from projects`}
              >
                <Trash2 size={13} aria-hidden="true" />
              </button>
            </div>
          );
        })}
      </nav>

      <div className="mt-4 border-t border-subtle pt-3 space-y-2">
        <label className="block text-xs font-medium text-secondary" htmlFor="project-profile">Add configured profile</label>
        <div className="flex gap-1.5">
          <select
            id="project-profile"
            value={profileToAdd}
            onChange={(event) => setProfileToAdd(event.target.value)}
            disabled={loading || saving || available.length === 0}
            className="min-w-0 flex-1 rounded-md border border-subtle bg-raised px-2 py-1.5 text-xs text-primary disabled:opacity-50"
          >
            {available.length === 0 && <option value="">All profiles added</option>}
            {available.map((profile) => <option key={profile.name} value={profile.name}>{profile.display_name || profile.name}</option>)}
          </select>
          <button type="button" onClick={addExisting} disabled={loading || saving || !profileToAdd} className="btn-secondary !min-h-0 !px-2" aria-label="Add configured profile">
            <Plus size={14} aria-hidden="true" />
          </button>
        </div>

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
            <button type="button" onClick={importProject} disabled={loading || saving || !gitUrl.trim()} className="btn-primary w-full !min-h-0 text-xs">
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
