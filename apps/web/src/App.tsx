import { lazy, Suspense, useState } from 'react';
import { LoadingState } from './components/ui/EmptyState.js';
import { useWebSocket } from './ws/WebSocketContext.js';
import { OverviewPage } from './pages/OverviewPage.js';
const WorkPage = lazy(() => import('./pages/WorkPage.js').then((module) => ({ default: module.WorkPage })));
const TelemetryPage = lazy(() => import('./pages/TelemetryPage.js').then((module) => ({ default: module.TelemetryPage })));
const QuotaPage = lazy(() => import('./pages/QuotaPage.js').then((module) => ({ default: module.QuotaPage })));
const EventsPage = lazy(() => import('./pages/EventsPage.js').then((module) => ({ default: module.EventsPage })));
const SettingsPage = lazy(() => import('./pages/SettingsPage.js').then((module) => ({ default: module.SettingsPage })));
const ManagerChatPage = lazy(() => import('./pages/ManagerChatPage.js').then((module) => ({ default: module.ManagerChatPage })));
const GitPage = lazy(() => import('./pages/GitPage.js').then((module) => ({ default: module.GitPage })));
import { Navbar } from './components/Navbar.js';
import { ConnectionStatus } from './components/ConnectionStatus.js';
import { SessionDetailModal } from './components/SessionDetailModal.js';
import type { Session } from '@git-agent-harness/contracts';

// Overview / Work / Telemetry / Quota / Events / Settings -- consolidated
// from the task's preferred 8-section nav. "Models" is a tab within
// Telemetry (same report data, same page) rather than a separate route;
// "Reviews" is exposed inside a work item's attempt timeline (a review is
// just a ledger entry tied to a work_id) rather than its own top-level
// data-fetch layer. See the final report for the full rationale.
export type Page = 'overview' | 'work' | 'telemetry' | 'quota' | 'events' | 'settings' | 'chat' | 'git';

export function App() {
  const [currentPage, setCurrentPage] = useState<Page>('overview');
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const { isConnected, isConnecting, error: wsError, sessions, serverVersion } = useWebSocket();

  const renderPage = () => {
    switch (currentPage) {
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
    <div className="min-h-screen bg-page lg:flex">
      <a href="#main-content" className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 bg-card text-primary p-3 rounded-md">Skip to content</a>
      <Navbar currentPage={currentPage} onPageChange={setCurrentPage} />

      <div className="flex-1 min-w-0">
        <div className="hidden lg:flex items-center justify-end px-6 py-2 border-b border-subtle">
          <ConnectionStatus
            isConnected={isConnected}
            isConnecting={isConnecting}
            error={wsError}
            serverVersion={serverVersion}
          />
        </div>

        <main id="main-content" tabIndex={-1} className="px-4 py-4 sm:px-6 sm:py-6 max-w-[1400px] mx-auto">
          <div className="lg:hidden mb-4">
            <ConnectionStatus
              isConnected={isConnected}
              isConnecting={isConnecting}
              error={wsError}
              serverVersion={serverVersion}
            />
          </div>
          <Suspense fallback={<LoadingState label="Loading page…" />}>{renderPage()}</Suspense>
        </main>
      </div>

      {selectedSession && (
        <SessionDetailModal session={selectedSession} onClose={() => setSelectedSession(null)} />
      )}
    </div>
  );
}
