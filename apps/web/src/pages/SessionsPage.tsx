import React from 'react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { SessionCard } from '../components/SessionCard.js';
import type { Session } from '@git-agent-harness/contracts';

type SessionsPageProps = {
  sessions: Session[];
  onSelectSession: (session: Session) => void;
  isConnected: boolean;
};

export function SessionsPage({ 
  sessions, 
  onSelectSession, 
  isConnected 
}: SessionsPageProps) {
  const { sendMessage } = useWebSocket();

  const handleStartSession = () => {
    // This would open a modal to start a new session
    // For now, just demonstrate the WebSocket message
    sendMessage({
      type: 'session.start',
      requestId: `req_${Date.now()}`,
      providerKind: 'github',
      instanceId: 'github_instance_0',
      repo: 'owner/repo',
      mode: 'improve',
      backend: 'claude',
      model: 'claude-3-sonnet',
      budget: 10
    });
  };

  const handleRefreshSessions = () => {
    sendMessage({
      type: 'provider.list',
      requestId: `req_${Date.now()}`
    });
  };

  const activeSessions = sessions.filter(s => ['starting', 'running'].includes(s.status));
  const finishedSessions = sessions.filter(s => ['stopped', 'error'].includes(s.status));

  return (
    <div className="space-y-6">
      <div className="flex justify-between items-center">
        <h2 className="text-2xl font-bold text-gray-900">Sessions</h2>
        <div className="flex space-x-3">
          <button 
            onClick={handleRefreshSessions}
            className="btn btn-secondary"
          >
            Refresh
          </button>
          <button 
            onClick={handleStartSession}
            className="btn btn-primary"
            disabled={!isConnected}
          >
            Start Session
          </button>
        </div>
      </div>

      <div className="space-y-6">
        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-4">
            Active Sessions ({activeSessions.length})
          </h3>
          
          {activeSessions.length === 0 ? (
            <div className="card text-center py-8">
              <p className="text-gray-500">No active sessions</p>
            </div>
          ) : (
            <div className="space-y-4">
              {activeSessions.map(session => (
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
            Recent Sessions ({finishedSessions.slice(0, 5).length})
          </h3>
          
          {finishedSessions.length === 0 ? (
            <div className="card text-center py-8">
              <p className="text-gray-500">No recent sessions</p>
            </div>
          ) : (
            <div className="space-y-4">
              {finishedSessions.slice(0, 5).map(session => (
                <SessionCard 
                  key={session.id} 
                  session={session} 
                  onClick={() => onSelectSession(session)} 
                />
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}