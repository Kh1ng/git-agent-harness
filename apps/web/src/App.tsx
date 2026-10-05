import { lazy, Suspense, useEffect, useMemo, useState } from 'react';
import { LoadingState } from './components/ui/EmptyState.js';
import { useWebSocket } from './ws/WebSocketContext.js';
import { OverviewPage } from './pages/OverviewPage.js';
import { ActivityBar, Navbar, PageTabs } from './components/Navbar.js';
import { PwaStatusBars } from './components/PwaStatusBars.js';
import { SessionDetailModal } from './components/SessionDetailModal.js';
import type { Session } from '@git-agent-harness/contracts';
import { isSideView, readNavigation, takeActivityDeepLink, updateNavigation, type MainPage, type Page, type SideView } from './lib/navigationState.js';
import { activityApi, gahApi } from './api/client.js';
import type { DeviceAgentsSnapshot } from '@git-agent-harness/contracts';
import { NotificationsMenu } from './components/NotificationsMenu.js';
import { SubscriptionUsageMenu } from './components/SubscriptionUsageMenu.js';
import { busySubscriptionIds, subscriptionUsage } from './lib/subscriptionUsage.js';
import { useGahStore } from './store/gahStore.js';
import { Maximize2, PanelRight, X } from 'lucide-react';
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
const PlanningPage = lazy(() => import('./pages/PlanningPage.js').then((module) => ({ default: module.PlanningPage })));
const IssuesPanel = lazy(() => import('./pages/IssuesPanel.js').then((module) => ({ default: module.IssuesPanel })));
const ProfilePanel = lazy(() => import('./pages/ProfilePanel.js').then((module) => ({ default: module.ProfilePanel })));

export type { Page } from './lib/navigationState.js';

/** From this width the sidebar opens beside the main panel; below it, an open
 * sidebar view takes the whole content area. */
const SPLIT_LAYOUT = '(min-width: 1280px)';

const SIDE_VIEW_LABELS: Record<SideView, string> = { events: 'Activity', issues: 'Git issues', profile: 'Profile', settings: 'Settings' };

/** The navbar's subscription rings follow the quota snapshot at this cadence. */
const QUOTA_REFRESH_MS = 5 * 60 * 1000;
const DEVICE_AGENTS_REFRESH_MS = 10 * 1000;

export function App() {
  const [currentPage, setCurrentPage] = useState<MainPage>(() => readNavigation().page);
  const [sideView, setSideView] = useState<SideView | null>(() => (new URLSearchParams(window.location.hash.slice(1)).has('pair') || new URLSearchParams(window.location.search).has('pairingRequest')) ? 'settings' : readNavigation().side);
  // Chat lives in the right sidebar; `currentPage === 'chat'` is the same chat expanded over the main panel.
  const [chatDocked, setChatDocked] = useState(() => readNavigation().dock === 'chat');
  const [pageBehindChat, setPageBehindChat] = useState<MainPage>('overview');
  useEffect(() => updateNavigation({ page: currentPage, side: sideView, dock: chatDocked && currentPage !== 'chat' ? 'chat' : null }), [currentPage, sideView, chatDocked]);
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [selectedWorkId, setSelectedWorkId] = useState<string | null>(null);
  // The Git issues sidebar grows while it shows an issue's detail; Overview opens its work there.
  const [sideDetailOpen, setSideDetailOpen] = useState(false);
  const [sideWorkId, setSideWorkId] = useState<string | null>(null);
  useEffect(() => { if (sideView !== 'issues') setSideWorkId(null); }, [sideView]);
  const openWorkInSidebar = (workId: string) => { setSideWorkId(workId); setSideView('issues'); };
  // A push or feed link names the notification it opened; opening it reads it.
  const [openedActivityId] = useState(takeActivityDeepLink);
  useEffect(() => {
    if (openedActivityId) void activityApi.markRead([openedActivityId]).catch(() => { /* It stays unread and visible. */ });
  }, [openedActivityId]);
  const profileOverride = useUiStore((state) => state.profileOverride);
  const requestAction = useUiStore((state) => state.requestAction);
  const notificationPopups = useUiStore((state) => state.notificationPopups);
  const { isConnected, isConnecting, sessions, liveActivity, activityUnreadCount, profile, sendMessage, activityRevision, reconnectSeq, controllerActivity } = useWebSocket();
  // The quota snapshot feeds the navbar's subscription rings on every page.
  const quota = useGahStore((state) => state.quota);
  const fetchQuota = useGahStore((state) => state.fetchQuota);
  const activeProfile = profileOverride ?? profile ?? undefined;
  useEffect(() => {
    fetchQuota({ profile: activeProfile, since: '7d' });
    const timer = window.setInterval(() => fetchQuota({ profile: activeProfile, since: '7d' }, { force: true }), QUOTA_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [activeProfile, fetchQuota, reconnectSeq]);
  const subscriptions = useMemo(() => subscriptionUsage(quota.data), [quota.data]);
  const statusSnapshot = useGahStore((state) => state.status.data);
  // The device's agent processes: what the factory is running right now, and what runs beside it.
  const [deviceAgents, setDeviceAgents] = useState<{ data: DeviceAgentsSnapshot | null; error: string | null }>({ data: null, error: null });
  useEffect(() => {
    let current = true;
    const load = () => gahApi.getDeviceAgents()
      .then((data) => { if (current) setDeviceAgents({ data, error: null }); })
      .catch((err) => { if (current) setDeviceAgents((state) => ({ data: state.data, error: err instanceof Error ? err.message : String(err) })); });
    void load();
    const timer = window.setInterval(load, DEVICE_AGENTS_REFRESH_MS);
    return () => { current = false; window.clearInterval(timer); };
  }, [reconnectSeq]);
  const busySubscriptions = useMemo(() => busySubscriptionIds({ subscriptions, sessions, controllerRuns: controllerActivity, claims: statusSnapshot?.active_claims ?? [], recentLedger: statusSnapshot?.recent_ledger, factoryAgents: deviceAgents.data?.factory_agents }),
    [subscriptions, sessions, controllerActivity, statusSnapshot, deviceAgents.data]);

  /** Pages link to each other by name; a sidebar view opens in the sidebar. */
  const navigate = (page: Page) => {
    if (isSideView(page)) return setSideView(page);
    const split = window.matchMedia(SPLIT_LAYOUT).matches;
    if (!split) { setSideView(null); setChatDocked(false); }
    // Beside the main panel when there is room; an expanded chat stays expanded.
    if (page === 'chat' && split && currentPage !== 'chat') return setChatDocked(true);
    setCurrentPage(page);
  };
  const toggleChat = () => {
    // Expanded, the Chat button docks the chat back beside the main panel.
    if (currentPage === 'chat') { if (window.matchMedia(SPLIT_LAYOUT).matches) expandChat(false); }
    else if (chatDocked) setChatDocked(false);
    else navigate('chat');
  };
  const expandChat = (expanded: boolean) => {
    if (expanded) setPageBehindChat(currentPage === 'chat' ? pageBehindChat : currentPage);
    setCurrentPage(expanded ? 'chat' : pageBehindChat);
    setChatDocked(!expanded);
  };
  const redispatch = ({ profile, repo, workId, backend }: { profile: string; repo: string; workId: string; backend: Parameters<typeof generateProviderInstanceId>[0] }) => sendMessage({
    type: 'session.start',
    requestId: `redispatch_${Date.now()}`,
    profile,
    providerKind: backend,
    instanceId: generateProviderInstanceId(backend, 0),
    repo,
    mode: 'fix',
    backend,
    target: workId,
  });
  const workDetail = (workId: string, onClose: () => void, inline = false) => (
    <WorkDetailDrawer workId={workId} profile={profileOverride ?? profile ?? 'gah'} connected={isConnected} sessions={sessions}
      onClose={onClose} onRedispatch={redispatch} inline={inline} />
  );
  const chat = (docked: boolean) => (
    <ManagerChatPage docked={docked} onNavigate={navigate} onOpenWork={setSelectedWorkId} />
  );

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
        return chat(false);
      case 'git':
        return <GitPage />;
      case 'planning':
        return <PlanningPage onNavigate={navigate} />;
      case 'overview':
      default:
        return (
          <OverviewPage
            sessions={sessions}
            deviceAgents={deviceAgents}
            onNavigate={navigate}
            onOpenWork={openWorkInSidebar}
          />
        );
    }
  };

  const isChatPage = currentPage === 'chat';

  return (
    <div className="app-shell flex h-dvh flex-col overflow-hidden bg-page">
      <a href="#main-content" className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 bg-card text-primary p-3 rounded-md">Skip to content</a>
      <PwaStatusBars />
      <Navbar currentPage={currentPage} sideView={sideView} onPageChange={navigate} activityUnreadCount={activityUnreadCount}
        chatOpen={isChatPage || chatDocked} onChatToggle={toggleChat}
        onImportProject={() => { requestAction('import'); expandChat(true); }} onCreateProject={() => { requestAction('create'); setSideView('profile'); }}
        actions={<><SubscriptionUsageMenu subscriptions={subscriptions} busy={busySubscriptions} onOpenQuota={() => navigate('quota')} /><NotificationsMenu liveActivity={sideView === 'events' ? null : liveActivity} unreadCount={activityUnreadCount} revision={activityRevision} autoPopup={notificationPopups} /></>} />

      {/* Left to right: icon strip, sidebar view, main panel, chat sidebar. */}
      <div className="flex min-h-0 flex-1">
        <ActivityBar sideView={sideView} onToggle={(view) => setSideView(sideView === view ? null : view)} />

        {sideView && (
          <aside id="side-panel" aria-label={SIDE_VIEW_LABELS[sideView]}
            className={`side-panel min-w-0 flex-1 overflow-y-auto px-4 py-4 xl:flex-none xl:border-r xl:border-subtle ${sideDetailOpen ? 'xl:w-[clamp(28rem,40vw,44rem)]' : 'xl:w-[clamp(20rem,25vw,30rem)]'}`}>
            <Suspense fallback={<LoadingState label="Loading…" />}>
              {sideView === 'settings' ? <SettingsPage /> : sideView === 'profile' ? <ProfilePanel /> : sideView === 'issues' ? <IssuesPanel renderDetail={(workId, onBack) => workDetail(workId, onBack, true)} detailWorkId={sideWorkId} onSelectWork={setSideWorkId} onDetailChange={setSideDetailOpen} onOpenChat={() => navigate('chat')} /> : <EventsPage openedEventId={openedActivityId} />}
            </Suspense>
          </aside>
        )}

        <div className={`min-h-0 min-w-0 flex-1 flex-col ${sideView || chatDocked ? 'hidden xl:flex' : 'flex'} ${isChatPage ? '' : 'overflow-y-auto'}`}>
          <main id="main-content" tabIndex={-1} className={`mx-auto w-full max-w-[1400px] px-4 py-4 sm:px-6 sm:py-6 ${isChatPage ? 'flex min-h-0 flex-1 flex-col' : ''}`}>
            {isChatPage && (
              <button type="button" onClick={() => expandChat(false)} className="mb-2 hidden items-center gap-1.5 self-end text-xs text-secondary hover:text-primary xl:inline-flex">
                <PanelRight size={14} aria-hidden="true" />
                Dock chat to the side
              </button>
            )}
            {!isConnected && !isConnecting && sideView !== 'settings' && (
              <p role="status" className="mb-3 text-sm text-secondary">
                Disconnected. <button className="min-h-11 text-accent underline" onClick={() => setSideView('settings')}>Open connection settings</button>
              </p>
            )}
            {!isChatPage && <PageTabs currentPage={currentPage} onPageChange={navigate} />}
            <Suspense fallback={<LoadingState label="Loading page…" />}>{renderPage()}</Suspense>
          </main>
        </div>

        {chatDocked && !isChatPage && (
          <aside id="chat-panel" aria-label="Chat"
            className={`min-w-0 flex-1 flex-col px-3 py-2 [contain:layout] xl:w-[clamp(22rem,30vw,34rem)] xl:flex-none xl:border-l xl:border-subtle ${sideView ? 'hidden xl:flex' : 'flex'}`}>
            <div className="mb-2 flex shrink-0 items-center justify-between">
              <h2 className="text-xs font-semibold uppercase tracking-wide text-muted">Chat</h2>
              <div className="flex items-center">
                <button type="button" onClick={() => expandChat(true)} className="activity-bar-button !h-8 !w-8" aria-label="Expand chat" title="Expand chat"><Maximize2 size={15} aria-hidden="true" /></button>
                <button type="button" onClick={() => setChatDocked(false)} className="activity-bar-button !h-8 !w-8" aria-label="Close chat" title="Close chat"><X size={16} aria-hidden="true" /></button>
              </div>
            </div>
            <Suspense fallback={<LoadingState label="Loading chat…" />}>{chat(true)}</Suspense>
          </aside>
        )}
      </div>

      {selectedSession && (
        <SessionDetailModal session={selectedSession} onClose={() => setSelectedSession(null)} />
      )}
      {selectedWorkId && workDetail(selectedWorkId, () => setSelectedWorkId(null))}
    </div>
  );
}
