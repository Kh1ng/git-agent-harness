import { useEffect, useMemo, useRef, useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import type { ChatSessionProjectGroup, ChatSessionSummary } from '@git-agent-harness/contracts';

interface ChatSessionPickerProps {
  /** Every project's sessions (live + settled/archived), grouped server-side. */
  groups: ChatSessionProjectGroup[];
  selectedProfile: string;
  selectedSessionId: string | null;
  /** Picking a session of another project moves the whole chat page to that
   * project first (profileOverride), then opens the session. */
  onSelect: (profile: string, sessionId: string | null) => void;
}

/** Collapsed-group persistence: one localStorage entry per group key --
 * 'archive' for the archive section, the profile id per project. */
const COLLAPSED_PREFIX = 'gah.chatPicker.collapsed.';

function readCollapsed(key: string): boolean {
  try {
    return window.localStorage.getItem(COLLAPSED_PREFIX + key) === '1';
  } catch {
    return false;
  }
}

function writeCollapsed(key: string, collapsed: boolean): void {
  try {
    if (collapsed) window.localStorage.setItem(COLLAPSED_PREFIX + key, '1');
    else window.localStorage.removeItem(COLLAPSED_PREFIX + key);
  } catch {
    // Storage unavailable (private mode etc.): collapse state simply
    // doesn't survive a reload.
  }
}

interface SplitGroup extends ChatSessionProjectGroup {
  active: ChatSessionSummary[];
  archived: ChatSessionSummary[];
}

/** The chat session dropdown: active conversations grouped under their
 * project (each collapsible), then settled/archived ones under an Archive
 * section, also sorted by project. Sessions of other projects are
 * selectable and switch the chat page to that project. */
export function ChatSessionPicker({ groups, selectedProfile, selectedSessionId, onSelect }: ChatSessionPickerProps) {
  const [open, setOpen] = useState(false);
  const [collapsedOverrides, setCollapsedOverrides] = useState<Record<string, boolean>>({});
  const containerRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  const { activeGroups, archiveGroups } = useMemo(() => {
    const split = (group: ChatSessionProjectGroup): SplitGroup => ({
      ...group,
      active: group.sessions.filter((session) => session.outcome === 'live'),
      archived: group.sessions.filter((session) => session.outcome !== 'live')
    });
    const sorted = groups.map(split).sort((a, b) => a.profileName.localeCompare(b.profileName));
    return {
      activeGroups: sorted.filter((group) => group.active.length > 0),
      archiveGroups: sorted.filter((group) => group.archived.length > 0)
    };
  }, [groups]);

  const isCollapsed = (key: string) => collapsedOverrides[key] ?? readCollapsed(key);
  const toggleCollapsed = (key: string) => {
    const next = !isCollapsed(key);
    setCollapsedOverrides((prev) => ({ ...prev, [key]: next }));
    writeCollapsed(key, next);
  };

  useEffect(() => {
    if (!open) return;
    panelRef.current?.focus();
    const onPointerDown = (event: PointerEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [open]);

  const select = (profile: string, sessionId: string | null) => {
    onSelect(profile, sessionId);
    setOpen(false);
    triggerRef.current?.focus();
  };

  const handlePanelKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === 'Escape') {
      event.stopPropagation();
      setOpen(false);
      triggerRef.current?.focus();
      return;
    }
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    const options = Array.from(panelRef.current?.querySelectorAll<HTMLButtonElement>('[role="option"]') ?? []);
    if (options.length === 0) return;
    const index = options.indexOf(document.activeElement as HTMLButtonElement);
    const next = index === -1
      ? options[event.key === 'ArrowDown' ? 0 : options.length - 1]
      : options[(index + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length];
    next.focus();
  };

  const selectedSession = groups
    .flatMap((group) => group.sessions)
    .find((session) => session.profile === selectedProfile && session.id === selectedSessionId);
  const selectedLabel = selectedSessionId === null
    ? 'Default conversation'
    : selectedSession
      ? selectedSession.title ?? selectedSession.branch
      : selectedSessionId;

  const optionClasses = (selected: boolean) =>
    `w-full rounded-md px-2.5 py-1.5 text-left text-xs truncate ${
      selected ? 'bg-accent/15 text-primary' : 'text-secondary hover:bg-white/5'
    }`;

  const sessionOption = (group: SplitGroup, session: ChatSessionSummary) => {
    const selected = group.profile === selectedProfile && session.id === selectedSessionId;
    return (
      <button
        key={`${group.profile}#${session.id}`}
        type="button"
        role="option"
        aria-selected={selected}
        onClick={() => select(group.profile, session.id)}
        className={optionClasses(selected)}
      >
        {session.title ?? session.branch}
      </button>
    );
  };

  /** Project header + its sessions; the same collapse key controls the
   * project's group in the active and the archive section. */
  const projectGroup = (group: SplitGroup, sessions: ChatSessionSummary[]) => {
    const collapsed = isCollapsed(group.profile);
    return (
      <div key={group.profile} role="group" aria-label={group.profileName} className="space-y-0.5">
        <button
          type="button"
          onClick={() => toggleCollapsed(group.profile)}
          aria-expanded={!collapsed}
          aria-label={group.profileName}
          className="flex w-full items-center gap-1 rounded px-1.5 py-1 text-[11px] font-semibold uppercase tracking-wide text-muted hover:bg-white/5 hover:text-primary"
        >
          {collapsed
            ? <ChevronRight size={12} className="shrink-0" aria-hidden="true" />
            : <ChevronDown size={12} className="shrink-0" aria-hidden="true" />}
          <span className="truncate">{group.profileName}</span>
          <span className="ml-auto text-[10px] normal-case tracking-normal">{sessions.length}</span>
        </button>
        {!collapsed && sessions.map((session) => sessionOption(group, session))}
      </div>
    );
  };

  const archiveCollapsed = isCollapsed('archive');

  return (
    <div ref={containerRef} className="relative">
      <button
        ref={triggerRef}
        type="button"
        aria-label="Chat session"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            setOpen(true);
          }
        }}
        className="inline-flex min-w-[12rem] max-w-[16rem] items-center justify-between gap-2 rounded-md border border-subtle bg-raised px-2 py-1.5 text-left text-xs text-primary"
      >
        <span className="truncate">{selectedLabel}</span>
        <ChevronDown size={13} className="shrink-0 text-muted" aria-hidden="true" />
      </button>
      {open && (
        <div
          ref={panelRef}
          role="listbox"
          aria-label="All sessions"
          tabIndex={-1}
          onKeyDown={handlePanelKeyDown}
          className="absolute left-0 z-20 mt-1 max-h-96 w-72 space-y-1 overflow-y-auto rounded-md border border-subtle bg-raised p-1 shadow-lg focus:outline-none"
        >
          <button
            type="button"
            role="option"
            aria-selected={selectedSessionId === null}
            onClick={() => select(selectedProfile, null)}
            className={optionClasses(selectedSessionId === null)}
          >
            Default conversation
          </button>
          <div role="group" aria-label="Active" className="space-y-1">
            {activeGroups.map((group) => projectGroup(group, group.active))}
          </div>
          {archiveGroups.length > 0 && (
            <div role="group" aria-label="Archive" className="space-y-1 border-t border-subtle pt-1">
              <button
                type="button"
                onClick={() => toggleCollapsed('archive')}
                aria-expanded={!archiveCollapsed}
                aria-label="Archive"
                className="flex w-full items-center gap-1 rounded px-1.5 py-1 text-[11px] font-semibold uppercase tracking-wide text-muted hover:bg-white/5 hover:text-primary"
              >
                {archiveCollapsed
                  ? <ChevronRight size={12} className="shrink-0" aria-hidden="true" />
                  : <ChevronDown size={12} className="shrink-0" aria-hidden="true" />}
                <span className="truncate">Archive</span>
                <span className="ml-auto text-[10px] normal-case tracking-normal">
                  {archiveGroups.reduce((sum, group) => sum + group.archived.length, 0)}
                </span>
              </button>
              {!archiveCollapsed && archiveGroups.map((group) => projectGroup(group, group.archived))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
