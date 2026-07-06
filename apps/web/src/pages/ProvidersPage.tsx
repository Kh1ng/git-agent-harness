import React from 'react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';

export function ProvidersPage() {
  const { 
    providers, 
    providerStatuses,
    sendMessage,
    isConnected 
  } = useWebSocket();

  const handleRefreshProvider = (instanceId: string) => {
    if (isConnected) {
      sendMessage({
        type: 'provider.refresh',
        requestId: `req_${Date.now()}`,
        instanceId
      });
    }
  };

  const handleRefreshAll = () => {
    providers.forEach(provider => {
      handleRefreshProvider(provider.instanceId);
    });
  };

  // Group providers by category
  const gitProviders = providers.filter(p => ['github', 'gitlab'].includes(p.providerKind));
  const aiProviders = providers.filter(p => ['codex', 'claude', 'cursor', 'opencode', 'grok'].includes(p.providerKind));
  const agentProviders = providers.filter(p => ['openhands', 'agy', 'vibe'].includes(p.providerKind));

  return (
    <div className="space-y-6">
      <div className="flex justify-between items-center">
        <h2 className="text-2xl font-bold text-gray-900">Providers</h2>
        <button 
          onClick={handleRefreshAll}
          className="btn btn-secondary"
          disabled={!isConnected}
        >
          Refresh All
        </button>
      </div>

      <div className="space-y-6">
        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-3">
            Git Hosting
          </h3>
          <div className="space-y-3">
            {gitProviders.map(provider => (
              <div 
                key={provider.instanceId}
                className="flex items-center"
                onClick={() => handleRefreshProvider(provider.instanceId)}
              >
                <ProviderStatusCard 
                  provider={provider} 
                  status={providerStatuses[provider.instanceId]}
                />
              </div>
            ))}
          </div>
        </div>

        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-3">
            AI Coding Assistants
          </h3>
          <div className="space-y-3">
            {aiProviders.map(provider => (
              <div 
                key={provider.instanceId}
                className="flex items-center"
                onClick={() => handleRefreshProvider(provider.instanceId)}
              >
                <ProviderStatusCard 
                  provider={provider} 
                  status={providerStatuses[provider.instanceId]}
                />
              </div>
            ))}
          </div>
        </div>

        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-3">
            Agent Frameworks
          </h3>
          <div className="space-y-3">
            {agentProviders.map(provider => (
              <div 
                key={provider.instanceId}
                className="flex items-center"
                onClick={() => handleRefreshProvider(provider.instanceId)}
              >
                <ProviderStatusCard 
                  provider={provider} 
                  status={providerStatuses[provider.instanceId]}
                />
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}