import React, { useState } from 'react';
import { useWebSocket } from './ws/WebSocketContext.js';
import { SessionsPage } from './pages/SessionsPage.js';
import { ProvidersPage } from './pages/ProvidersPage.js';
import { DashboardPage } from './pages/DashboardPage.js';
import { Navbar } from './components/Navbar.js';
import { ConnectionStatus } from './components/ConnectionStatus.js';
import { SessionDetailModal } from './components/SessionDetailModal.js';
import type { Session } from '@git-agent-harness/contracts';

export type Page = 'dashboard' | 'sessions' | 'providers';

export function App() {
  const [currentPage, setCurrentPage] = useState<Page>('dashboard');
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const { 
    isConnected, 
    isConnecting, 
    error: wsError,
    sessions,
    serverVersion 
  } = useWebSocket();

  const handleSelectSession = (session: Session) => {
    setSelectedSession(session);
  };

  const handleCloseSessionDetail = () => {
    setSelectedSession(null);
  };

  const renderPage = () => {
    switch (currentPage) {
      case 'sessions':
        return (
          <SessionsPage 
            sessions={sessions} 
            onSelectSession={handleSelectSession} 
            isConnected={isConnected} 
          />
        );
      case 'providers':
        return <ProvidersPage />;
      case 'dashboard':
      default:
        return (
          <DashboardPage 
            sessions={sessions} 
            onSelectSession={handleSelectSession} 
            isConnected={isConnected}
          />
        );
    }
  };

  return (
    <div className="min-h-screen bg-gray-50">
      <Navbar currentPage={currentPage} onPageChange={setCurrentPage} />
      
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-4">
        <ConnectionStatus 
          isConnected={isConnected} 
          isConnecting={isConnecting} 
          error={wsError} 
          serverVersion={serverVersion}
        />
        
        <main className="mt-6">
          {renderPage()}
        </main>
      </div>

      {selectedSession && (
        <SessionDetailModal 
          session={selectedSession} 
          onClose={handleCloseSessionDetail} 
        />
      )}
    </div>
  );
}