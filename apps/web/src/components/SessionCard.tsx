import React from 'react';
import type { Session } from '@git-agent-harness/contracts';

type SessionCardProps = {
  session: Session;
  onClick: () => void;
};

const statusColors: Record<string, string> = {
  idle: 'bg-gray-100 text-gray-800',
  starting: 'bg-yellow-100 text-yellow-800',
  running: 'bg-green-100 text-green-800',
  stopping: 'bg-yellow-100 text-yellow-800',
  stopped: 'bg-gray-100 text-gray-800',
  error: 'bg-red-100 text-red-800',
};

const providerIcons: Record<string, string> = {
  github: '💻',
  gitlab: '🦊',
  codex: '🤖',
  claude: '🎯',
  cursor: '✨',
  opencode: '🔓',
  grok: '🌟',
  openhands: '🤝',
  agy: '🧠',
  vibe: '⚡',
  auto: '🎲',
};

const modeIcons: Record<string, string> = {
  improve: '✨',
  pm: '📋',
  review: '👀',
  fix: '🔧',
  experiment: '🧪',
};

export function SessionCard({ session, onClick }: SessionCardProps) {
  const statusColor = statusColors[session.status] || 'bg-gray-100 text-gray-800';
  const providerIcon = providerIcons[session.providerKind] || '📦';
  const modeIcon = modeIcons[session.mode] || '🎯';

  return (
    <div 
      onClick={onClick}
      className="card session-card cursor-pointer transition-shadow"
    >
      <div className="flex items-start justify-between">
        <div className="flex-1">
          <div className="flex items-center space-x-3">
            <span className="text-2xl">{providerIcon}</span>
            <div>
              <h3 className="text-lg font-semibold text-gray-900">
                {session.repo}
              </h3>
              <p className="text-sm text-gray-500">
                {session.id}
              </p>
            </div>
          </div>
          
          <div className="mt-4">
            <div className="flex items-center space-x-4 text-sm">
              <span className="flex items-center">
                <span className="mr-1">{modeIcon}</span>
                <span className="text-gray-600">{session.mode}</span>
              </span>
              
              {session.branch && (
                <span className="flex items-center">
                  <span className="mr-1">🌿</span>
                  <span className="text-gray-600">{session.branch}</span>
                </span>
              )}
              
              {session.backend && (
                <span className="flex items-center">
                  <span className="mr-1">⚙️</span>
                  <span className="text-gray-600">{session.backend}</span>
                </span>
              )}
              
              {session.model && (
                <span className="flex items-center">
                  <span className="mr-1">🧠</span>
                  <span className="text-gray-600">{session.model}</span>
                </span>
              )}
            </div>
          </div>
        </div>
        
        <div className="ml-4">
          <span className={`badge ${statusColor}`}>
            {session.status}
          </span>
        </div>
      </div>
      
      {session.error && (
        <div className="mt-3 p-2 bg-red-50 rounded-md">
          <p className="text-sm text-red-700">{session.error}</p>
        </div>
      )}
    </div>
  );
}