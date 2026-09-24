import { useEffect, useRef, useState } from 'react';
import { Info, Loader2, Upload } from 'lucide-react';
import type { Skill, SkillCreateData, SkillSummary } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { skillFromFrontMatter, SkillFrontMatterError } from '../lib/skillFrontMatter.js';
import { EmptyState } from './ui/EmptyState.js';
import { StatusBadge } from './ui/StatusBadge.js';

interface SkillDraft {
  id: string;
  version: string;
  displayName: string;
  description: string;
  backendText: string;
  content: string;
  source: string;
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function draftFromSkill(skill: Skill): SkillDraft {
  return { ...skill, backendText: skill.backends.join(', ') };
}

function skillFromDraft(draft: SkillDraft): SkillCreateData {
  return {
    id: draft.id,
    version: draft.version,
    displayName: draft.displayName.trim(),
    description: draft.description.trim(),
    backends: [...new Set(draft.backendText.split(',').map(value => value.trim()).filter(Boolean))],
    content: draft.content,
    source: draft.source
  };
}

/** Owner editor for versioned skill records. Read access remains available to paired controllers. */
export function SkillBankSettingsSection() {
  const [skills, setSkills] = useState<SkillSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploaded, setUploaded] = useState<string | null>(null);
  const [editing, setEditing] = useState<SkillSummary | null>(null);
  const [draft, setDraft] = useState<SkillDraft | null>(null);
  const [editorError, setEditorError] = useState<string | null>(null);
  const [editorNotice, setEditorNotice] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const load = async () => {
    try {
      const { skills: loaded } = await gahApi.getSkills();
      setSkills(loaded);
      setError(null);
    } catch (failure) {
      setError(message(failure));
    }
  };

  useEffect(() => { void load(); }, []);
  useWsReconnectRefresh(load);

  const handleFileSelected = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    setUploading(true);
    setUploadError(null);
    setUploaded(null);
    try {
      const text = await file.text();
      const created = await gahApi.createSkill(skillFromFrontMatter(file.name, text));
      setUploaded(`${created.id}@${created.version}`);
      await load();
    } catch (failure) {
      setUploadError(failure instanceof SkillFrontMatterError ? failure.message : message(failure));
    } finally {
      setUploading(false);
    }
  };

  const openEditor = async (skill: SkillSummary) => {
    setEditing(skill);
    setDraft(null);
    setEditorError(null);
    setEditorNotice(null);
    try {
      setDraft(draftFromSkill(await gahApi.getSkill(skill.id, skill.version)));
    } catch (failure) {
      setEditorError(message(failure));
    }
  };

  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!draft || !editing) return;
    if (!draft.displayName.trim() || !draft.content.trim()) {
      setEditorError('Display name and Markdown content cannot be empty.');
      return;
    }
    setSaving(true);
    setEditorError(null);
    setEditorNotice(null);
    try {
      const saved = await gahApi.createSkill(skillFromDraft(draft));
      setDraft(draftFromSkill(saved));
      setEditing({ ...editing, ...saved });
      setEditorNotice(`Saved ${saved.id}@${saved.version}.`);
      await load();
    } catch (failure) {
      setEditorError(message(failure));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!editing || editing.bound) return;
    if (!window.confirm(`Remove '${editing.id}' and all installed versions? This cannot be undone.`)) return;
    setSaving(true);
    setEditorError(null);
    setEditorNotice(null);
    try {
      await gahApi.deleteSkill(editing.id);
      setEditing(null);
      setDraft(null);
      setEditorNotice(`Removed '${editing.id}' and all installed versions.`);
      await load();
    } catch (failure) {
      setEditorError(message(failure));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="card-padded">
      <div className="mb-1 flex flex-wrap items-start justify-between gap-3">
        <h3 className="text-sm font-semibold text-primary">Central skill bank</h3>
        <div>
          <input
            ref={fileInputRef}
            type="file"
            accept=".md"
            onChange={handleFileSelected}
            className="hidden"
            aria-label="Upload SKILL.md"
          />
          <button
            type="button"
            onClick={() => fileInputRef.current?.click()}
            disabled={uploading}
            className="btn-secondary inline-flex gap-1.5 px-3 py-1.5 text-xs disabled:opacity-50"
          >
            {uploading ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <Upload size={14} aria-hidden="true" />}
            {uploading ? 'Uploading…' : 'Upload SKILL.md'}
          </button>
        </div>
      </div>
      <p className="mb-3 text-xs text-muted">
        Owners can upload and edit versioned skills here. Bind skills to projects from Manager Chat.
      </p>
      {uploadError && <p role="alert" className="mb-3 text-xs text-critical">Failed to upload skill: {uploadError}</p>}
      {uploaded && !uploadError && <p role="status" className="mb-3 text-xs text-good">Uploaded {uploaded}.</p>}
      {editorNotice && <p role="status" className="mb-3 text-xs text-good">{editorNotice}</p>}
      {error ? (
        <p className="text-xs text-critical">Failed to load skills: {error}</p>
      ) : skills === null ? (
        <p className="text-xs text-muted">Loading…</p>
      ) : skills.length === 0 ? (
        <EmptyState icon={Info} title="No skills installed" description="Upload a SKILL.md to add the first versioned skill." />
      ) : (
        <div className="divide-y divide-subtle rounded-md border border-subtle">
          {skills.map(skill => {
            const selected = editing?.id === skill.id && editing.version === skill.version;
            return <div key={`${skill.id}@${skill.version}`} className="p-3 text-xs">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium text-primary">{skill.displayName}</span>
                    <code className="font-mono text-muted">{skill.id}@{skill.version}</code>
                    {skill.bound && <StatusBadge tone="good" label="bound" />}
                  </div>
                  {skill.description && <p className="mt-1 break-words text-muted">{skill.description}</p>}
                  <p className="mt-1 break-all text-muted">
                    {skill.backends.length > 0 ? `Backends: ${skill.backends.join(', ')}` : 'All backends'} · {skill.source}
                  </p>
                </div>
                <button
                  type="button"
                  className="btn-secondary shrink-0 px-2.5 py-1 text-xs"
                  aria-expanded={selected}
                  disabled={editing !== null}
                  onClick={() => void openEditor(skill)}
                >
                  {selected ? 'Editing' : 'Edit'} <span className="sr-only">{skill.displayName} {skill.version}</span>
                </button>
              </div>

              {selected && (
                <div className="mt-3 border-t border-subtle pt-3">
                  {editorError && <p role="alert" className="mb-3 text-xs text-critical">{editorError}</p>}
                  {!draft ? (
                    editorError
                      ? <button type="button" className="btn-secondary" onClick={() => { setEditing(null); setEditorError(null); }}>Close editor</button>
                      : <p role="status" className="text-xs text-muted">Loading skill…</p>
                  ) : (
                    <form className="space-y-3" onSubmit={save}>
                      <div className="grid gap-3 sm:grid-cols-2">
                        <label className="space-y-1 font-medium text-secondary">
                          <span>Display name</span>
                          <input
                            className="input w-full"
                            value={draft.displayName}
                            onChange={event => setDraft({ ...draft, displayName: event.target.value })}
                          />
                        </label>
                        <label className="space-y-1 font-medium text-secondary">
                          <span>Backend compatibility</span>
                          <input
                            className="input w-full font-mono"
                            value={draft.backendText}
                            onChange={event => setDraft({ ...draft, backendText: event.target.value })}
                            placeholder="All backends"
                            aria-describedby="skill-backends-help"
                          />
                        </label>
                      </div>
                      <p id="skill-backends-help" className="text-muted">Comma-separated backend IDs. Leave blank for all backends.</p>
                      <label className="block space-y-1 font-medium text-secondary">
                        <span>Description</span>
                        <textarea
                          className="input min-h-20 w-full resize-y"
                          value={draft.description}
                          onChange={event => setDraft({ ...draft, description: event.target.value })}
                        />
                      </label>
                      <label className="block space-y-1 font-medium text-secondary">
                        <span>Markdown content</span>
                        <textarea
                          className="input min-h-72 w-full resize-y font-mono text-xs leading-5"
                          value={draft.content}
                          onChange={event => setDraft({ ...draft, content: event.target.value })}
                          spellCheck={false}
                        />
                      </label>
                      <p className="text-muted">Editing {draft.id}@{draft.version}. The source remains {draft.source}.</p>
                      <div className="flex flex-wrap items-center gap-2">
                        <button type="submit" className="btn-primary" disabled={saving}>
                          {saving ? 'Saving…' : 'Save changes'}
                        </button>
                        <button type="button" className="btn-secondary" disabled={saving} onClick={() => { setEditing(null); setDraft(null); setEditorError(null); }}>
                          Cancel
                        </button>
                        <button
                          type="button"
                          className="btn-secondary text-critical"
                          disabled={saving || editing.bound}
                          onClick={() => void remove()}
                        >
                          Remove skill
                        </button>
                      </div>
                      {editing.bound && (
                        <p className="text-muted">Removal is unavailable while this skill is bound. Unbind it from every project and backend first.</p>
                      )}
                    </form>
                  )}
                </div>
              )}
            </div>;
          })}
        </div>
      )}
    </section>
  );
}
