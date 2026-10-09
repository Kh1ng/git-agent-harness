import { useEffect, useRef, useState } from 'react';
import { Check, ChevronDown, FolderGit2, FolderPlus, GitBranch } from 'lucide-react';
import { useGahStore } from '../store/gahStore.js';
import { useUiStore } from '../store/uiStore.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { updateNavigation } from '../lib/navigationState.js';

/**
 * "Project: <name>" at the far left of the navbar. Opening it lists every
 * configured project (a GAH profile) to switch the whole dashboard to, plus
 * Import from Git (the chat's project rail import form) and Create new (the
 * Profile sidebar's add form).
 */
export function ProjectSwitcher({ onImport, onCreate }: { onImport: () => void; onCreate: () => void }) {
  const { profile: wsProfile, serverVersion } = useWebSocket();
  const profileOverride = useUiStore((state) => state.profileOverride);
  const setProfileOverride = useUiStore((state) => state.setProfileOverride);
  const profiles = useGahStore((state) => state.profiles);
  const fetchProfiles = useGahStore((state) => state.fetchProfiles);
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLDivElement>(null);

  useEffect(() => { fetchProfiles(); }, [fetchProfiles]);
  useEffect(() => {
    if (!open) return;
    fetchProfiles();
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent ? event.key === 'Escape' : !menu.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => { document.removeEventListener('mousedown', close); document.removeEventListener('keydown', close); };
  }, [open, fetchProfiles]);

  const configured = profiles.data ?? [];
  const selectedName = profileOverride ?? wsProfile ?? '';
  const selected = configured.find((candidate) => candidate.name === selectedName);
  // Until the server's welcome arrives the project is unknown, not absent.
  const label = selected?.display_name || selectedName || (serverVersion === null ? 'loading…' : 'none');

  const choose = (name: string) => {
    setProfileOverride(name);
    updateNavigation({ profile: name, chat: null, epic: null, map: null });
    setOpen(false);
  };

  return (
    <div className="relative shrink-0" ref={menu}>
      <button type="button" onClick={() => setOpen(!open)} aria-expanded={open} aria-haspopup="menu"
        className="flex max-w-[16rem] items-center gap-1.5 rounded-md px-2 py-1.5 text-sm hover:bg-overlay/5" title={selected?.repo ?? label}>
        <FolderGit2 size={15} className="shrink-0 text-accent" aria-hidden="true" />
        <span className="truncate"><span className="text-muted">Project: </span><span className="font-semibold text-primary">{label}</span></span>
        <ChevronDown size={14} className="shrink-0 text-muted" aria-hidden="true" />
      </button>
      {open && (
        <div className="absolute left-0 top-full z-40 mt-1 w-72 max-w-[calc(100vw-1.5rem)] rounded-lg border border-subtle bg-raised shadow-xl" role="menu" aria-label="Projects">
          <div className="max-h-72 overflow-y-auto py-1">
            {profiles.loading && configured.length === 0 && <p className="px-3 py-2 text-xs text-muted">Loading projects…</p>}
            {profiles.error && <p role="alert" className="px-3 py-2 text-xs text-critical">Cannot load projects: {profiles.error}</p>}
            {!profiles.loading && !profiles.error && configured.length === 0 && <p className="px-3 py-2 text-xs text-muted">No project configured yet.</p>}
            {configured.map((candidate) => {
              const active = candidate.name === selectedName;
              return (
                <button key={candidate.name} type="button" role="menuitemradio" aria-checked={active} onClick={() => choose(candidate.name)}
                  className={`flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-overlay/5 ${active ? 'bg-accent/10' : ''}`}>
                  <span className="flex h-4 w-4 shrink-0 items-center justify-center">{active && <Check size={14} className="text-accent" aria-hidden="true" />}</span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm text-primary">{candidate.display_name || candidate.name}</span>
                    <span className="block truncate text-[11px] text-muted">{candidate.repo}{candidate.name !== (candidate.display_name || candidate.name) ? ` · ${candidate.name}` : ''}</span>
                  </span>
                </button>
              );
            })}
          </div>
          <div className="flex gap-2 border-t border-subtle p-2">
            <button type="button" role="menuitem" onClick={() => { setOpen(false); onImport(); }} className="btn-secondary flex-1 text-xs">
              <GitBranch size={13} aria-hidden="true" /> Import from Git
            </button>
            <button type="button" role="menuitem" onClick={() => { setOpen(false); onCreate(); }} className="btn-secondary flex-1 text-xs">
              <FolderPlus size={13} aria-hidden="true" /> Create new
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
