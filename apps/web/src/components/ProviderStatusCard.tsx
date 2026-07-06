import React from 'react';
import type { ProviderInstance, ProviderStatus } from '@git-agent-harness/contracts';

type ProviderStatusCardProps = {
  provider: ProviderInstance;
  status: ProviderStatus | undefined;
};

const statusColors: Record<string, string> = {
  unavailable: 'bg-gray-100 text-gray-800',
  available: 'bg-green-100 text-green-800',
  authenticated: 'bg-blue-100 text-blue-800',
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

export function ProviderStatusCard({ provider, status }: ProviderStatusCardProps) {
  const icon = providerIcons[provider.providerKind] || '📦';
  const statusColor = status ? statusColors[status.type] || 'bg-gray-100 text-gray-800' : 'bg-gray-100 text-gray-800';

  return (
    <div className="card p-4 cursor-pointer transition-shadow hover:shadow-md">
      <div className="flex items-center justify-between">
        <div className="flex items-center space-x-3">
          <span className="text-xl">{icon}</span>
          <div>
            <p className="font-medium text-gray-900">{provider.name}</p>
            <p className="text-xs text-gray-500">{provider.providerKind}</p>
          </div>
        </div>
        
        <div className="flex items-center space-x-2">
          <span className={`badge ${statusColor}`}>
            {status ? status.type : 'unknown'}
          </span>
          {status?.type === 'authenticated' && status.userId && (
            <span className="text-xs text-gray-500">{status.userId}</span>
          )}
        </div>
      </div>
    </div>
  );
}