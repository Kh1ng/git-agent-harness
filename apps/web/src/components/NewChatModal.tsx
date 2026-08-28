import { useEffect, useState } from 'react';
import { FolderGit2, Server, Cpu, X } from 'lucide-react';
import type { ChatNodeInfo, ManagerModelInfo, ProfileSummary } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import type { ManagerBackendInfo } from '@git-agent-harness/contracts';

interface NewChatModalProps {
  open: boolean;
  currentProfile: string;
  profiles: ProfileSummary[];
  backends: ManagerBackendInfo[];
  onClose: () => void;
  onCreated: (profile: string, sessionId: string) => void;
}

/**
 * The T3-style new-chat flow: choose a project, choose a node, choose a
 * provider/model. The created conversation is bound to a fresh worktree —
 * every backend in that session runs in the same directory, and the branch
 * survives archive/reclaim.
 */
export function NewChatModal({ open, currentProfile, profiles, backends, onClose, onCreated }: NewChatModalProps) {
  const [project, setProject] = useState(currentProfile);
  const [nodes, setNodes] = useState<ChatNodeInfo[]>([]);
  const [backend, setBackend] = useState<string>('');
  const [models, setModels] = useState<ManagerModelInfo[]>([]);
  const [model, setModel] = useState<string | null>(null);
  const [title, setTitle] = useState('');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const implementedBackends = backends.filter((b) => b.implemented);

  useEffect(() => {
    if (!open) return;
    setProject(currentProfile);
    setError(null);
    setTitle('');
    setModel(null);
    gahApi
      .getChatNodes()
      .then(({ nodes }) => setNodes(nodes))
      .catch(() => setNodes([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!open) return;
    // Default to the profile's configured backend, else the first implemented one.
    gahApi
      .getManagerChatSettings()
      .then((settings) => {
        const preferred = settings.profileOverrides[project] ?? settings.defaultBackend;
        setBackend(implementedBackends.some((b) => b.id === preferred) ? preferred : implementedBackends[0]?.id ?? '');
      })
      .catch(() => setBackend(implementedBackends[0]?.id ?? ''));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, project]);

  useEffect(() => {
    if (!open || !backend) {
      setModels([]);
      setModel(null);
      return;
    }
    let cancelled = false;
    setModels([]);
    setModel(null);
    gahApi
      .getManagerChatModelsForBackend(project, backend)
      .then(({ models, currentModelId }) => {
        if (cancelled) return;
        setModels(models);
        setModel(models.length > 0 ? currentModelId ?? models[0].id : null);
      })
      .catch(() => {
        if (!cancelled) {
          setModels([]);
          setModel(null);
        }
      });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, project, backend]);

  if (!open) return null;

  const central = nodes.find((n) => n.role === 'central' && n.chatCapable);
  const workers = nodes.filter((n) => n !== central);

  const create = async () => {
    if (!backend) return;
    setCreating(true);
    setError(null);
    try {
      const session = await gahApi.createChatSession(project, backend, model, title.trim() || undefined);
      onCreated(project, session.id);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" role="dialog" aria-modal="true" aria-label="New chat">
      <div className="card w-full max-w-lg max-h-[85vh] overflow-y-auto p-5 space-y-5">
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold text-primary">New chat</h2>
          <button onClick={onClose} className="rounded p-1 text-muted hover:bg-white/5 hover:text-primary" aria-label="Close">
            <X size={16} aria-hidden="true" />
          </button>
        </div>

        <section className="space-y-2">
          <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
            <FolderGit2 size={13} aria-hidden="true" /> Project
          </h3>
          <div className="grid gap-1">
            {profiles.map((p) => (
              <button
                key={p.name}
                type="button"
                onClick={() => setProject(p.name)}
                className={`rounded-md px-3 py-2 text-left ${project === p.name ? 'bg-accent/15 border border-accent/40' : 'border border-transparent hover:bg-white/5'}`}
              >
                <span className="block text-sm font-medium text-primary">{p.display_name || p.name}</span>
                <span className="block text-[11px] text-muted truncate">{p.repo}</span>
              </button>
            ))}
            {profiles.length === 0 && <p className="text-xs text-muted">No configured profiles. Import one from the rail below.</p>}
          </div>
        </section>

        <section className="space-y-2">
          <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
            <Server size={13} aria-hidden="true" /> Node
          </h3>
          {central && (
            <div className="rounded-md border border-accent/40 bg-accent/10 px-3 py-2 flex items-center justify-between">
              <div>
                <span className="block text-sm font-medium text-primary">{central.displayName}</span>
                <span className="block text-[11px] text-muted">central node · chat runs here</span>
              </div>
              <span className="text-[10px] uppercase tracking-wide rounded bg-accent/20 text-primary px-1.5 py-0.5">selected</span>
            </div>
          )}
          {workers.map((n) => (
            <div key={n.nodeId} className="rounded-md border border-subtle px-3 py-2 flex items-center justify-between opacity-60">
              <div>
                <span className="block text-sm font-medium text-secondary">{n.displayName}</span>
                <span className="block text-[11px] text-muted">worker · chat on workers coming soon</span>
              </div>
            </div>
          ))}
          {!central && <p className="text-xs text-muted">Central node unavailable — chat will still start on this server.</p>}
        </section>

        <section className="space-y-2">
          <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted">
            <Cpu size={13} aria-hidden="true" /> Provider / model
          </h3>
          <div className="grid grid-cols-3 gap-1.5">
            {implementedBackends.map((b) => (
              <button
                key={b.id}
                type="button"
                onClick={() => setBackend(b.id)}
                className={`rounded-md px-2 py-2 text-sm ${backend === b.id ? 'bg-accent/15 border border-accent/40 text-primary' : 'border border-subtle text-secondary hover:bg-white/5'}`}
              >
                {b.displayName}
              </button>
            ))}
          </div>
          {models.length > 0 && (
            <select
              value={model ?? ''}
              onChange={(e) => setModel(e.target.value || null)}
              className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-xs text-primary"
              aria-label="Model"
            >
              {models.map((m) => (
                <option key={m.id} value={m.id}>{m.name}</option>
              ))}
            </select>
          )}
          {backend && models.length === 0 && (
            <p className="text-[11px] text-muted">This provider uses its default model.</p>
          )}
        </section>

        <section className="space-y-2">
          <label htmlFor="new-chat-title" className="block text-xs font-medium text-secondary">Title (optional)</label>
          <input
            id="new-chat-title"
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="e.g. Fix the retry loop"
            className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-xs text-primary"
          />
        </section>

        {error && <p className="text-xs text-red-400">{error}</p>}

        <div className="flex justify-end gap-2 pt-1">
          <button type="button" onClick={onClose} className="btn-secondary text-xs">Cancel</button>
          <button
            type="button"
            onClick={create}
            disabled={creating || !backend || profiles.length === 0}
            className="btn-primary text-xs"
          >
            {creating ? 'Creating…' : 'Start chat'}
          </button>
        </div>
      </div>
    </div>
  );
}
