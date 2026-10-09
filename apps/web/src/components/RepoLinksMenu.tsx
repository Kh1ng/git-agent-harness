import { useEffect, useRef, useState } from 'react';
import { ChevronDown, CircleDot, ExternalLink, GitPullRequest } from 'lucide-react';
import { useGahStore } from '../store/gahStore.js';
import { useUiStore } from '../store/uiStore.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { ExternalAnchor } from './ExternalAnchor.js';

/** The provider's issue and review pages for a repository URL. */
export function repoPages(webUrl: string, provider: string | null | undefined): { label: string; issues: string; reviews: string; reviewsLabel: string } {
  const base = webUrl.replace(/\/+$/, '');
  if (provider === 'gitlab' || /gitlab\./.test(base)) {
    return { label: 'GitLab', issues: `${base}/-/issues`, reviews: `${base}/-/merge_requests`, reviewsLabel: 'Merge requests' };
  }
  return { label: 'GitHub', issues: `${base}/issues`, reviews: `${base}/pulls`, reviewsLabel: 'Pull requests' };
}

/** A dropdown next to the project switcher that opens the current project's Issues and Pull requests on the provider. */
export function RepoLinksMenu() {
  const { profile: wsProfile } = useWebSocket();
  const profileOverride = useUiStore((state) => state.profileOverride);
  const profiles = useGahStore((state) => state.profiles);
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent ? event.key === 'Escape' : !menu.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => { document.removeEventListener('mousedown', close); document.removeEventListener('keydown', close); };
  }, [open]);

  const selectedName = profileOverride ?? wsProfile ?? '';
  const selected = profiles.data?.find((candidate) => candidate.name === selectedName);
  if (!selected?.web_url) return null;
  const pages = repoPages(selected.web_url, selected.provider);

  return (
    <div className="relative shrink-0" ref={menu}>
      <button type="button" onClick={() => setOpen(!open)} aria-expanded={open} aria-haspopup="menu" aria-label={pages.label}
        className="flex items-center gap-1 rounded-md px-2 py-1.5 text-sm text-secondary hover:bg-overlay/5 hover:text-primary" title={selected.repo}>
        <ExternalLink size={14} aria-hidden="true" />
        <span className="hidden sm:inline">{pages.label}</span>
        <ChevronDown size={14} className="text-muted" aria-hidden="true" />
      </button>
      {open && (
        <div className="absolute left-0 top-full z-40 mt-1 w-56 rounded-lg border border-subtle bg-raised py-1 shadow-xl" role="menu" aria-label={`${pages.label} pages`}>
          <ExternalAnchor href={pages.issues} role="menuitem" onClick={() => setOpen(false)} className="flex items-center gap-2 px-3 py-2 text-sm text-primary hover:bg-overlay/5">
            <CircleDot size={14} className="text-muted" aria-hidden="true" /> Issues
          </ExternalAnchor>
          <ExternalAnchor href={pages.reviews} role="menuitem" onClick={() => setOpen(false)} className="flex items-center gap-2 px-3 py-2 text-sm text-primary hover:bg-overlay/5">
            <GitPullRequest size={14} className="text-muted" aria-hidden="true" /> {pages.reviewsLabel}
          </ExternalAnchor>
          <p className="truncate px-3 pt-1 text-[11px] text-muted" title={selected.repo}>{selected.repo}</p>
        </div>
      )}
    </div>
  );
}
