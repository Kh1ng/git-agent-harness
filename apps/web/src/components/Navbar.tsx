import { useState } from 'react';
import {
  LayoutDashboard,
  ListChecks,
  BarChart3,
  Gauge,
  Radio,
  Settings,
  Menu,
  X,
  Server,
  Activity
} from 'lucide-react';
import type { Page } from '../App.js';
import { useWebSocket } from '../ws/WebSocketContext.js';

function HostList() {
  const { hosts, hostsStatus, sessions } = useWebSocket();

  if (!hosts || hosts.length === 0) return null;

  return (
    <div className="mt-6 pt-6 border-t border-subtle">
      <h3 className="px-2 text-xs font-semibold text-muted uppercase tracking-wider mb-2 flex items-center gap-1.5">
        <Server size={12} />
        Connected Hosts
      </h3>
      <div className="space-y-1">
        {hosts.map((host) => {
          const status = hostsStatus[host.id];
          const isReachable = status?.reachable ?? true;
          
          let activeCount = status?.activeSessionCount;
          if (activeCount === undefined) {
            activeCount = sessions.filter(s => {
              const isRunning = s.status === 'running' || s.status === 'starting';
              if (host.id === 'local') {
                return isRunning && (!s.hostId || s.hostId === 'local');
              }
              return isRunning && s.hostId === host.id;
            }).length;
          }

          return (
            <div
              key={host.id}
              className="flex items-center justify-between px-2 py-1.5 rounded-md text-xs hover:bg-muted/50 transition-colors"
            >
              <div className="flex items-center gap-2 min-w-0">
                <span className={`h-2 w-2 rounded-full shrink-0 ${isReachable ? 'bg-green-500' : 'bg-red-500'}`} />
                <span className="font-medium truncate text-primary">{host.name}</span>
              </div>
              {activeCount > 0 && (
                <span className="flex items-center gap-1 px-1.5 py-0.5 rounded-full bg-accent/10 text-accent font-semibold text-[10px]">
                  <Activity size={10} />
                  {activeCount}
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

type NavbarProps = {
  currentPage: Page;
  onPageChange: (page: Page) => void;
};

export const FRONTEND_BUILD = `v${__GAH_VERSION__} (${__GAH_COMMIT__})`;

const navItems: { id: Page; label: string; icon: typeof LayoutDashboard }[] = [
  { id: 'overview', label: 'Overview', icon: LayoutDashboard },
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
        <HostList />
      </aside>

      {/* Mobile top bar */}
      <header className="lg:hidden sticky top-0 z-30 flex items-center justify-between h-14 px-4 bg-card border-b border-subtle">
        <div>
          <h1 className="text-sm font-semibold text-primary">Git Agent Harness</h1>
          <p className="text-[10px] text-muted font-mono" data-testid="frontend-build">{FRONTEND_BUILD}</p>
        </div>
        <button
          onClick={() => setDrawerOpen(true)}
          className="btn-secondary !px-2"
          aria-label="Open navigation menu"
          aria-expanded={drawerOpen}
        >
          <Menu size={18} aria-hidden="true" />
        </button>
      </header>

      {/* Mobile drawer */}
      {drawerOpen && (
        <div className="lg:hidden fixed inset-0 z-40">
          <div
            className="absolute inset-0 bg-black/60"
            onClick={() => setDrawerOpen(false)}
            aria-hidden="true"
          />
          <div className="absolute inset-y-0 left-0 w-72 max-w-[85vw] bg-card border-r border-subtle p-3 flex flex-col">
            <div className="flex items-center justify-between px-2 py-3 mb-2">
              <div>
                <h1 className="text-sm font-semibold text-primary">Git Agent Harness</h1>
                <p className="text-[10px] text-muted font-mono" data-testid="frontend-build">{FRONTEND_BUILD}</p>
              </div>
              <button
                onClick={() => setDrawerOpen(false)}
                className="btn-secondary !px-2"
                aria-label="Close navigation menu"
              >
                <X size={18} aria-hidden="true" />
              </button>
            </div>
            <NavLinks currentPage={currentPage} onSelect={handleSelect} />
            <HostList />
          </div>
        </div>
      )}
    </>
  );
}
