import { lazy, Suspense, useEffect, useState } from 'react';
import { LoadingState } from './components/ui/EmptyState.js';
import { useWebSocket } from './ws/WebSocketContext.js';
import { OverviewPage } from './pages/OverviewPage.js';
import { Navbar } from './components/Navbar.js';
import { PwaStatusBars } from './components/PwaStatusBars.js';
import { SessionDetailModal } from './components/SessionDetailModal.js';
import type { Session } from '@git-agent-harness/contracts';
import { readNavigation, updateNavigation, type Page } from './lib/navigationState.js';
import { ActivityToast } from './components/ActivityToast.js';

const WorkPage = lazy(() => import('./pages/WorkPage.js').then((module) => ({ default: module.WorkPage })));
const TelemetryPage = lazy(() => import('./pages/TelemetryPage.js').then((module) => ({ default: module.TelemetryPage })));
const QuotaPage = lazy(() => import('./pages/QuotaPage.js').then((module) => ({ default: module.QuotaPage })));
const EventsPage = lazy(() => import('./pages/EventsPage.js').then((module) => ({ default: module.EventsPage })));
const SettingsPage = lazy(() => import('./pages/SettingsPage.js').then((module) => ({ default: module.SettingsPage })));
const ManagerChatPage = lazy(() => import('./pages/ManagerChatPage.js').then((module) => ({ default: module.ManagerChatPage })));
const GitPage = lazy(() => import('./pages/GitPage.js').then((module) => ({ default: module.GitPage })));
const NodesPage = lazy(() => import('./pages/NodesPage.js').then((module) => ({ default: module.NodesPage })));

export type { Page } from './lib/navigationState.js';

export function App() {
  const [currentPage, setCurrentPage] = useState<Page>(() => new URLSearchParams(window.location.hash.slice(1)).has('pair') ? 'settings' : readNavigation().page);
  useEffect(() => updateNavigation({ page: currentPage }), [currentPage]);
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [dismissedActivityId, setDismissedActivityId] = useState<string | null>(null);
  const { isConnected, isConnecting, sessions, liveActivity, activityUnreadCount } = useWebSocket();

  const renderPage = () => {
    switch (currentPage) {
      case 'nodes':
        return <NodesPage />;
      case 'work':
        return <WorkPage sessions={sessions} onSelectSession={setSelectedSession} />;
      case 'telemetry':
        return <TelemetryPage />;
      case 'quota':
        return <QuotaPage />;
      case 'events':
        return <EventsPage />;
      case 'settings':
        return <SettingsPage />;
      case 'chat':
        return <ManagerChatPage />;
      case 'git':
        return <GitPage />;
      case 'overview':
      default:
        return (
          <OverviewPage
            sessions={sessions}
            onSelectSession={setSelectedSession}
            onNavigate={setCurrentPage}
          />
        );
    }
  };

  return (
    <div className="app-shell min-h-dvh bg-page lg:flex">
      <a href="#main-content" className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 bg-card text-primary p-3 rounded-md">Skip to content</a>
      <PwaStatusBars />
      <Navbar currentPage={currentPage} onPageChange={setCurrentPage} activityUnreadCount={activityUnreadCount} />

      <div className="flex-1 min-w-0">
        <main id="main-content" tabIndex={-1} className="px-4 py-4 sm:px-6 sm:py-6 max-w-[1400px] mx-auto">
          {!isConnected && !isConnecting && currentPage !== 'settings' && (
            <p role="status" className="mb-3 text-sm text-secondary">
              Disconnected. <button className="min-h-11 text-accent underline" onClick={() => setCurrentPage('settings')}>Open connection settings</button>
            </p>
          )}
          <Suspense fallback={<LoadingState label="Loading page…" />}>{renderPage()}</Suspense>
        </main>
      </div>

      {selectedSession && (
        <SessionDetailModal session={selectedSession} onClose={() => setSelectedSession(null)} />
      )}
      {liveActivity && liveActivity.id !== dismissedActivityId && currentPage !== 'events' && (
        <ActivityToast
          event={liveActivity}
          onOpen={() => setCurrentPage('events')}
          onDismiss={() => setDismissedActivityId(liveActivity.id)}
        />
      )}
    </div>
  );
}
