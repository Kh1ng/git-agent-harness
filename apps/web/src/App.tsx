import { lazy, Suspense, useEffect, useState } from 'react';
import { LoadingState } from './components/ui/EmptyState.js';
import { useWebSocket } from './ws/WebSocketContext.js';
import { OverviewPage } from './pages/OverviewPage.js';
import { ActivityBar, Navbar } from './components/Navbar.js';
import { PwaStatusBars } from './components/PwaStatusBars.js';
import { SessionDetailModal } from './components/SessionDetailModal.js';
import { activityPath, type Session } from '@git-agent-harness/contracts';
import { isSideView, readNavigation, takeActivityDeepLink, updateNavigation, type MainPage, type Page, type SideView } from './lib/navigationState.js';
import { activityApi, gahApi } from './api/client.js';
import { ActivityToast } from './components/ActivityToast.js';
import { WorkDetailDrawer } from './components/WorkDetailDrawer.js';
import { generateProviderInstanceId } from '@git-agent-harness/shared';
import { useUiStore } from './store/uiStore.js';

const WorkPage = lazy(() => import('./pages/WorkPage.js').then((module) => ({ default: module.WorkPage })));
const TelemetryPage = lazy(() => import('./pages/TelemetryPage.js').then((module) => ({ default: module.TelemetryPage })));
const QuotaPage = lazy(() => import('./pages/QuotaPage.js').then((module) => ({ default: module.QuotaPage })));
const EventsPage = lazy(() => import('./pages/EventsPage.js').then((module) => ({ default: module.EventsPage })));
const SettingsPage = lazy(() => import('./pages/SettingsPage.js').then((module) => ({ default: module.SettingsPage })));
const ManagerChatPage = lazy(() => import('./pages/ManagerChatPage.js').then((module) => ({ default: module.ManagerChatPage })));
const GitPage = lazy(() => import('./pages/GitPage.js').then((module) => ({ default: module.GitPage })));
const NodesPage = lazy(() => import('./pages/NodesPage.js').then((module) => ({ default: module.NodesPage })));
const ProjectsPage = lazy(() => import('./pages/ProjectsPage.js').then((module) => ({ default: module.ProjectsPage })));
const PlanningPage = lazy(() => import('./pages/PlanningPage.js').then((module) => ({ default: module.PlanningPage })));

export type { Page } from './lib/navigationState.js';

/** From this width the sidebar opens beside the main panel; below it, an open
 * sidebar view takes the whole content area. */
const SPLIT_LAYOUT = '(min-width: 1280px)';

/** A standalone install is a central node with no worker nodes registered.
 * The last answer is remembered so the navbar doesn't flicker on load; an
 * unknown or unreadable registry keeps Nodes visible. */
function useStandalone(reconnectSeq: number, activityRevision: number): boolean {
  const [standalone, setStandalone] = useState(() => window.localStorage.getItem('gah-standalone') === 'true');
  useEffect(() => {
    let current = true;
    gahApi.getFleetSnapshot().then((fleet) => {
      if (!current) return;
      const next = fleet.nodes.length === 0;
      window.localStorage.setItem('gah-standalone', String(next));
      setStandalone(next);
    }).catch(() => { /* Keep the last known answer. */ });
    return () => { current = false; };
  }, [reconnectSeq, activityRevision]);
  return standalone;
}

export function App() {
  const [currentPage, setCurrentPage] = useState<MainPage>(() => readNavigation().page);
  const [sideView, setSideView] = useState<SideView | null>(() => (new URLSearchParams(window.location.hash.slice(1)).has('pair') || new URLSearchParams(window.location.search).has('pairingRequest')) ? 'settings' : readNavigation().side);
  const [chatLauncherRequest, setChatLauncherRequest] = useState(0);
  useEffect(() => updateNavigation({ page: currentPage, side: sideView }), [currentPage, sideView]);
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [selectedWorkId, setSelectedWorkId] = useState<string | null>(null);
  const [dismissedActivityId, setDismissedActivityId] = useState<string | null>(null);
  // A push or feed link names the notification it opened; opening it reads it.
  const [openedActivityId] = useState(takeActivityDeepLink);
  useEffect(() => {
    if (openedActivityId) void activityApi.markRead([openedActivityId]).catch(() => { /* It stays unread and visible. */ });
  }, [openedActivityId]);
  const profileOverride = useUiStore((state) => state.profileOverride);
  const showNodes = useUiStore((state) => state.showNodes);
  const { isConnected, isConnecting, sessions, liveActivity, activityUnreadCount, profile, sendMessage, reconnectSeq, activityRevision } = useWebSocket();
  const standalone = useStandalone(reconnectSeq, activityRevision);

  /** Pages link to each other by name; a sidebar view opens in the sidebar. */
  const navigate = (page: Page) => {
    if (isSideView(page)) return setSideView(page);
    setCurrentPage(page);
    if (!window.matchMedia(SPLIT_LAYOUT).matches) setSideView(null);
  };

  const renderPage = () => {
    switch (currentPage) {
      case 'nodes':
        return <NodesPage />;
      case 'work':
        return <WorkPage sessions={sessions} onSelectSession={setSelectedSession} onOpenWork={setSelectedWorkId} />;
      case 'telemetry':
        return <TelemetryPage />;
      case 'quota':
        return <QuotaPage />;
      case 'chat':
        return <ManagerChatPage launcherRequest={chatLauncherRequest} onNavigate={navigate} onOpenWork={setSelectedWorkId} />;
      case 'projects':
        return <ProjectsPage onNavigate={navigate} />;
      case 'git':
        return <GitPage />;
      case 'planning':
        return <PlanningPage onNavigate={navigate} />;
      case 'overview':
      default:
        return (
          <OverviewPage
            sessions={sessions}
            onSelectSession={setSelectedSession}
            onNavigate={navigate}
            onOpenWork={setSelectedWorkId}
          />
        );
    }
  };

  const isChatPage = currentPage === 'chat';
  const handlePrimaryNavigation = (page: Page) => {
    if (page === 'chat') setChatLauncherRequest((request) => request + 1);
    navigate(page);
  };

  return (
    <div className="app-shell flex h-dvh flex-col overflow-hidden bg-page">
      <a href="#main-content" className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 bg-card text-primary p-3 rounded-md">Skip to content</a>
      <PwaStatusBars />
      <Navbar currentPage={currentPage} sideView={sideView} onPageChange={handlePrimaryNavigation} hideNodes={standalone && !showNodes} activityUnreadCount={activityUnreadCount} />

      {/* Left to right: icon strip, sidebar view, main panel. A right sidebar belongs after the main panel. */}
      <div className="flex min-h-0 flex-1">
        <ActivityBar sideView={sideView} onToggle={(view) => setSideView(sideView === view ? null : view)} activityUnreadCount={activityUnreadCount} />

        {sideView && (
          <aside id="side-panel" aria-label={sideView === 'settings' ? 'Settings' : 'Activity'}
            className="side-panel min-w-0 flex-1 overflow-y-auto px-4 py-4 xl:w-[clamp(20rem,25vw,30rem)] xl:flex-none xl:border-r xl:border-subtle">
            <Suspense fallback={<LoadingState label="Loading…" />}>
              {sideView === 'settings' ? <SettingsPage /> : <EventsPage openedEventId={openedActivityId} />}
            </Suspense>
          </aside>
        )}

        <div className={`min-h-0 min-w-0 flex-1 flex-col ${sideView ? 'hidden xl:flex' : 'flex'} ${isChatPage ? '' : 'overflow-y-auto'}`}>
          <main id="main-content" tabIndex={-1} className={`mx-auto w-full max-w-[1400px] px-4 py-4 sm:px-6 sm:py-6 ${isChatPage ? 'flex min-h-0 flex-1 flex-col' : ''}`}>
            {!isConnected && !isConnecting && sideView !== 'settings' && (
              <p role="status" className="mb-3 text-sm text-secondary">
                Disconnected. <button className="min-h-11 text-accent underline" onClick={() => setSideView('settings')}>Open connection settings</button>
              </p>
            )}
            <Suspense fallback={<LoadingState label="Loading page…" />}>{renderPage()}</Suspense>
          </main>
        </div>
      </div>

      {selectedSession && (
        <SessionDetailModal session={selectedSession} onClose={() => setSelectedSession(null)} />
      )}
      {selectedWorkId && (
        <WorkDetailDrawer
          workId={selectedWorkId}
          profile={profileOverride ?? profile ?? 'gah'}
          connected={isConnected}
          sessions={sessions}
          onClose={() => setSelectedWorkId(null)}
          onRedispatch={({ profile, repo, workId, backend }) => sendMessage({
            type: 'session.start',
            requestId: `redispatch_${Date.now()}`,
            profile,
            providerKind: backend,
            instanceId: generateProviderInstanceId(backend, 0),
            repo,
            mode: 'fix',
            backend,
            target: workId,
          })}
        />
      )}
      {liveActivity && liveActivity.id !== dismissedActivityId && sideView !== 'events' && (
        <ActivityToast
          event={liveActivity}
          onOpen={() => window.location.assign(activityPath(liveActivity))}
          onDismiss={() => setDismissedActivityId(liveActivity.id)}
        />
      )}
    </div>
  );
}
