import { useEffect, useRef, useState } from 'react';
import {
  LayoutDashboard,
  ListChecks,
  BarChart3,
  Gauge,
  Radio,
  Settings,
  Menu,
  X,
  MessageSquare,
  GitBranch,
  Server
} from 'lucide-react';
import type { Page } from '../App.js';

type NavbarProps = {
  currentPage: Page;
  onPageChange: (page: Page) => void;
};

export const FRONTEND_BUILD = `v${__GAH_VERSION__} (${__GAH_COMMIT__})`;

const navItems: { id: Page; label: string; icon: typeof LayoutDashboard }[] = [
  { id: 'overview', label: 'Overview', icon: LayoutDashboard },
  { id: 'chat', label: 'Chat', icon: MessageSquare },
  { id: 'git', label: 'Git', icon: GitBranch },
  { id: 'nodes', label: 'Nodes', icon: Server },
  { id: 'work', label: 'Work', icon: ListChecks },
  { id: 'telemetry', label: 'Telemetry', icon: BarChart3 },
  { id: 'quota', label: 'Quota', icon: Gauge },
  { id: 'events', label: 'Events', icon: Radio },
  { id: 'settings', label: 'Settings', icon: Settings }
];

function NavLinks({ currentPage, onSelect }: { currentPage: Page; onSelect: (page: Page) => void }) {
  return (
    <nav className="flex flex-col gap-0.5" aria-label="Primary">
      {navItems.map((item) => {
        const Icon = item.icon;
        const active = currentPage === item.id;
        return (
          <button
            key={item.id}
            onClick={() => onSelect(item.id)}
            className={`nav-link ${active ? 'nav-link-active' : ''}`}
            aria-current={active ? 'page' : undefined}
          >
            <Icon size={17} aria-hidden="true" />
            {item.label}
          </button>
        );
      })}
    </nav>
  );
}

/** Desktop: fixed compact sidebar. Mobile (<1024px): a top bar with a
 * hamburger that opens a slide-in drawer -- never a permanently crushed
 * desktop sidebar. */
export function Navbar({ currentPage, onPageChange }: NavbarProps) {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const drawer = useRef<HTMLDialogElement>(null);

  // The native modal owns focus containment, Escape, and background inertness.
  useEffect(() => {
    if (drawerOpen) drawer.current?.showModal();
    else drawer.current?.close();
  }, [drawerOpen]);

  useEffect(() => {
    const desktop = window.matchMedia('(min-width: 1024px)');
    const closeOnDesktop = () => { if (desktop.matches) setDrawerOpen(false); };
    desktop.addEventListener('change', closeOnDesktop);
    return () => desktop.removeEventListener('change', closeOnDesktop);
  }, []);

  const handleSelect = (page: Page) => {
    onPageChange(page);
    setDrawerOpen(false);
  };

  return (
    <>
      {/* Desktop sidebar */}
      <aside className="hidden lg:flex lg:flex-col lg:w-56 lg:shrink-0 lg:border-r lg:border-subtle lg:bg-card lg:h-screen lg:sticky lg:top-0 lg:p-3">
        <div className="px-2 py-3 mb-2">
          <h1 className="text-sm font-semibold text-primary tracking-tight">Git Agent Harness</h1>
          <p className="text-xs text-muted mt-0.5">Control plane</p>
          <p className="text-[10px] text-muted mt-1 font-mono" data-testid="frontend-build">{FRONTEND_BUILD}</p>
        </div>
        <NavLinks currentPage={currentPage} onSelect={handleSelect} />
      </aside>

      {/* Mobile top bar */}
      <header className="mobile-app-header lg:hidden sticky top-0 z-30 flex items-center justify-between px-4 bg-card border-b border-subtle">
        <div>
          <h1 className="text-sm font-semibold text-primary">Git Agent Harness</h1>
        </div>
        <button
          onClick={() => setDrawerOpen(true)}
          className="btn-secondary !min-h-11 !min-w-11 !px-2"
          aria-label="Open navigation menu"
          aria-expanded={drawerOpen}
        >
          <Menu size={18} aria-hidden="true" />
        </button>
      </header>

      {/* Mobile drawer */}
      <dialog
        ref={drawer}
        aria-label="Navigation menu"
        onClose={() => setDrawerOpen(false)}
        onClick={(event) => {
          if (event.target !== event.currentTarget) return;
          const bounds = event.currentTarget.getBoundingClientRect();
          if (event.clientX < bounds.left || event.clientX > bounds.right ||
              event.clientY < bounds.top || event.clientY > bounds.bottom) {
            setDrawerOpen(false);
          }
        }}
        className="mobile-drawer m-0 h-dvh max-h-none w-72 max-w-[85vw] bg-card text-primary border-0 border-r border-subtle px-3 backdrop:bg-black/60"
      >
        <div className="flex items-center justify-between px-2 py-3 mb-2">
          <div>
            <h1 className="text-sm font-semibold text-primary">Git Agent Harness</h1>
            <p className="text-[10px] text-muted font-mono" data-testid="frontend-build">{FRONTEND_BUILD}</p>
          </div>
          <button
            onClick={() => setDrawerOpen(false)}
            className="btn-secondary !min-h-11 !min-w-11 !px-2"
            aria-label="Close navigation menu"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </div>
        <NavLinks currentPage={currentPage} onSelect={handleSelect} />
      </dialog>
    </>
  );
}
