import { useEffect, useRef, useState, type ReactNode } from 'react';
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
  Server,
  Orbit,
  FolderGit2,
  FolderCog,
  CircleDot,
  Bot
} from 'lucide-react';
import { ProjectSwitcher } from './ProjectSwitcher.js';
import { RepoLinksMenu } from './RepoLinksMenu.js';
import type { MainPage, Page, SideView } from '../lib/navigationState.js';

type NavItem<Id extends Page> = { id: Id; label: string; icon: typeof LayoutDashboard };
/** A navbar entry: its own page, plus the pages shown as tabs under it. */
type NavGroup = NavItem<MainPage> & { tabs?: NavItem<MainPage>[] };

type NavbarProps = {
  currentPage: MainPage;
  sideView: SideView | null;
  onPageChange: (page: Page) => void;
  activityUnreadCount?: number;
  /** Chat is open, docked in the right sidebar or filling the main panel. */
  chatOpen: boolean;
  /** Desktop: the Chat button opens and closes the right sidebar. */
  onChatToggle: () => void;
  /** Right-aligned controls shared by the desktop and phone bars. */
  actions?: ReactNode;
  /** The project switcher's Import from Git and Create new. */
  onImportProject: () => void;
  onCreateProject: () => void;
};

export const FRONTEND_BUILD = `v${__GAH_VERSION__} (${__GAH_COMMIT__})`;

/** The top navbar: each entry fills the main panel. A group opens on its
 * first page and lists the rest as tabs above the page. */
const mainItems: NavGroup[] = [
  { id: 'overview', label: 'Overview', icon: LayoutDashboard },
  { id: 'work', label: 'Factory', icon: ListChecks },
  { id: 'git', label: 'Projects', icon: FolderGit2, tabs: [
    { id: 'git', label: 'Git', icon: GitBranch },
    { id: 'planning', label: 'Planning', icon: Orbit }
  ] },
  { id: 'telemetry', label: 'Usage', icon: BarChart3, tabs: [
    { id: 'telemetry', label: 'Telemetry', icon: BarChart3 },
    { id: 'quota', label: 'Quota', icon: Gauge }
  ] },
  { id: 'nodes', label: 'Fleet', icon: Server }
];

const groupOf = (page: MainPage) => mainItems.find((group) => group.id === page || group.tabs?.some((tab) => tab.id === page));

/** The tabs shown above a page that belongs to a navbar group; none for a page on its own. */
export function pageTabs(page: MainPage): NavItem<MainPage>[] | null {
  return groupOf(page)?.tabs ?? null;
}

/** Tabs above the main panel for the pages a navbar group holds. */
export function PageTabs({ currentPage, onPageChange }: { currentPage: MainPage; onPageChange: (page: Page) => void }) {
  const tabs = pageTabs(currentPage);
  if (!tabs) return null;
  return (
    <nav aria-label="Page tabs" className="mb-4 flex gap-0.5 overflow-x-auto border-b border-subtle">
      {tabs.map((tab) => {
        const Icon = tab.icon;
        const active = currentPage === tab.id;
        return (
          <button key={tab.id} type="button" onClick={() => onPageChange(tab.id)}
            className={`top-nav-link ${active ? 'top-nav-link-active' : ''}`} aria-current={active ? 'page' : undefined}>
            <Icon size={14} aria-hidden="true" />
            {tab.label}
          </button>
        );
      })}
    </nav>
  );
}

/** Chat is not a main-panel tab on desktop: its button sits at the right of
 * the navbar and opens the right sidebar. The phone drawer lists it as a page. */
const chatItem: NavItem<MainPage> = { id: 'chat', label: 'Chat', icon: MessageSquare };

/** The left sidebar: each entry opens beside the main panel. */
const sideItems: NavItem<SideView>[] = [
  { id: 'events', label: 'Activity', icon: Radio },
  { id: 'issues', label: 'Git issues', icon: CircleDot },
  { id: 'agents', label: 'Running agents', icon: Bot },
  { id: 'profile', label: 'Profile', icon: FolderCog },
  { id: 'settings', label: 'Settings', icon: Settings }
];

function UnreadBadge({ count, className = '' }: { count: number; className?: string }) {
  if (count <= 0) return null;
  return (
    <span className={`min-w-5 rounded-full bg-accent px-1.5 py-0.5 text-center text-[10px] font-semibold text-page ${className}`} aria-label={`${count} unread`}>
      {Math.min(count, 99)}
    </span>
  );
}

/** Desktop: the collapsed sidebar is a thin strip of icons. An icon opens its
 * view beside the main panel; the same icon closes it again. */
export function ActivityBar({ sideView, onToggle }: {
  sideView: SideView | null;
  onToggle: (view: SideView) => void;
}) {
  return (
    <nav aria-label="Sidebar" className="hidden w-12 shrink-0 flex-col items-center gap-1 overflow-y-auto border-r border-subtle bg-card py-2 lg:flex">
      {sideItems.map((item) => {
        const Icon = item.icon;
        const open = sideView === item.id;
        return (
          <button
            key={item.id}
            type="button"
            onClick={() => onToggle(item.id)}
            className={`activity-bar-button ${open ? 'activity-bar-button-active' : ''} ${item.id === 'settings' ? 'mt-auto' : ''}`}
            aria-label={item.label}
            aria-expanded={open}
            aria-controls="side-panel"
            title={item.label}
          >
            <Icon size={20} aria-hidden="true" />
          </button>
        );
      })}
    </nav>
  );
}

/** Desktop: a top navbar for the main panel. Mobile (<1024px): the same bar
 * with a hamburger that opens a slide-in drawer listing every page. */
export function Navbar({ currentPage, sideView, onPageChange, activityUnreadCount = 0, chatOpen, onChatToggle, actions, onImportProject, onCreateProject }: NavbarProps) {
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
  const items = mainItems;
  // A group stays on the tab it is showing; its button only moves between groups.
  const selectGroup = (group: NavGroup) => { if (groupOf(currentPage) !== group) handleSelect(group.id); };

  return (
    <>
      <header className="mobile-app-header z-30 flex shrink-0 items-center gap-2 border-b border-subtle bg-card px-4 lg:min-h-0 lg:gap-4 lg:px-3">
        <h1 className="sr-only">Git Agent Harness</h1>
        <ProjectSwitcher onImport={onImportProject} onCreate={onCreateProject} />
        <nav className="hidden min-w-0 shrink items-stretch gap-0.5 overflow-x-auto lg:flex" aria-label="Primary">
          {items.map((item) => {
            const Icon = item.icon;
            const active = groupOf(currentPage) === item;
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => selectGroup(item)}
                className={`top-nav-link ${active ? 'top-nav-link-active' : ''}`}
                aria-current={active ? 'page' : undefined}
              >
                <Icon size={16} aria-hidden="true" />
                {item.label}
              </button>
            );
          })}
        </nav>
        {/* After Fleet: the repository's pages on the provider. */}
        <RepoLinksMenu />
        <div className="ml-auto flex shrink-0 items-center gap-1">
          <button type="button" onClick={onChatToggle} aria-pressed={chatOpen}
            className={`top-nav-link hidden rounded-md !border-b-0 !py-2 lg:flex ${chatOpen ? 'nav-link-active' : ''}`}>
            <MessageSquare size={16} aria-hidden="true" />
            Chat
          </button>
          {actions}
          <p className="hidden font-mono text-[10px] text-muted lg:block" data-testid="frontend-build">{FRONTEND_BUILD}</p>
          <button
            onClick={() => setDrawerOpen(true)}
            className="btn-secondary !min-h-11 !min-w-11 !px-2 lg:hidden"
            aria-label="Open navigation menu"
            aria-expanded={drawerOpen}
          >
            <Menu size={18} aria-hidden="true" />
          </button>
        </div>
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
        <nav className="flex flex-col gap-0.5" aria-label="Primary">
          {[...items, chatItem, ...sideItems].map((item: NavItem<Page>) => {
            const Icon = item.icon;
            const group = items.find((candidate) => candidate.id === item.id);
            const active = sideView ? sideView === item.id : (groupOf(currentPage) ?? { id: currentPage }).id === item.id;
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => { if (group) selectGroup(group); else handleSelect(item.id); setDrawerOpen(false); }}
                className={`nav-link ${active ? 'nav-link-active' : ''}`}
                aria-current={active ? 'page' : undefined}
              >
                <Icon size={17} aria-hidden="true" />
                {item.label}
                {item.id === 'events' && <UnreadBadge count={activityUnreadCount} className="ml-auto" />}
              </button>
            );
          })}
        </nav>
      </dialog>
    </>
  );
}
