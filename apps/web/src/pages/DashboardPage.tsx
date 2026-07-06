import React from 'react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { SessionCard } from '../components/SessionCard.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';
import type { Session } from '@git-agent-harness/contracts';

type DashboardPageProps = {
  sessions: Session[];
  onSelectSession: (session: Session) => void;
  isConnected: boolean;
};

export function DashboardPage({ 
  sessions, 
  onSelectSession, 
  isConnected 
}: DashboardPageProps) {
  const { providers, providerStatuses } = useWebSocket();

  return (
    <div className="space-y-6">
      <div className="flex justify-between items-center">
        <h2 className="text-2xl font-bold text-gray-900">Dashboard</h2>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-4">
            Active Sessions
          </h3>
          
          {sessions.length === 0 ? (
            <div className="card text-center py-8">
              <p className="text-gray-500">
                {isConnected ? 'No active sessions' : 'Connect to see active sessions'}
              </p>
            </div>
          ) : (
            <div className="space-y-4">
              {sessions.slice(0, 3).map(session => (
                <SessionCard 
                  key={session.id} 
                  session={session} 
                  onClick={() => onSelectSession(session)} 
                />
              ))}
            </div>
          )}
        </div>

        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-4">
            Provider Status
          </h3>
          
          <div className="space-y-3 max-h-96 overflow-y-auto">
            {providers.map(provider => (
              <ProviderStatusCard 
                key={provider.instanceId}
                provider={provider}
                status={providerStatuses[provider.instanceId]}
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}