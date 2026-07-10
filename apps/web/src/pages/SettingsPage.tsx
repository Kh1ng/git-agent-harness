import { useState } from 'react';
import { Sun, Moon, Info } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';

export function SettingsPage() {
  const { providers, providerStatuses, sendMessage, isConnected, serverVersion, profile } = useWebSocket();
  const { theme, setTheme, profileOverride, setProfileOverride } = useUiStore();
  const [profileInput, setProfileInput] = useState(profileOverride ?? '');

  const handleRefreshProvider = (instanceId: string) => {
    if (isConnected) {
      sendMessage({ type: 'provider.refresh', requestId: `req_${Date.now()}`, instanceId });
    }
  };

  return (
    <div className="space-y-6">
      <PageHeader title="Settings" description="Appearance, profile, and provider authentication" />

      <section className="card-padded max-w-md">
        <h3 className="text-sm font-semibold text-primary mb-3">Appearance</h3>
        <div className="flex rounded-md border border-subtle overflow-hidden w-fit text-sm">
          <button
            onClick={() => setTheme('dark')}
            className={`inline-flex items-center gap-1.5 px-3 py-1.5 ${theme === 'dark' ? 'bg-accent text-white' : 'text-secondary hover:bg-white/5'}`}
          >
            <Moon size={14} aria-hidden="true" />
            Dark
          </button>
          <button
            onClick={() => setTheme('light')}
            className={`inline-flex items-center gap-1.5 px-3 py-1.5 ${theme === 'light' ? 'bg-accent text-white' : 'text-secondary hover:bg-white/5'}`}
          >
            <Sun size={14} aria-hidden="true" />
            Light
          </button>
        </div>
      </section>

      <section className="card-padded max-w-md">
        <h3 className="text-sm font-semibold text-primary mb-2">Profile</h3>
        <p className="text-xs text-muted mb-3">
          Overrides which GAH profile Overview/Work/Telemetry/Quota/Events read from. Leave blank to use the
          server default (<span className="font-mono">{profile ?? 'gah'}</span>).
        </p>
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={profileInput}
            onChange={(e) => setProfileInput(e.target.value)}
            placeholder={profile ?? 'gah'}
            className="flex-1 bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary placeholder:text-muted"
          />
          <button onClick={() => setProfileOverride(profileInput.trim() || null)} className="btn-secondary">
            Apply
          </button>
        </div>
        <p className="mt-3 text-xs text-muted inline-flex items-start gap-1.5">
          <Info size={13} className="shrink-0 mt-0.5" aria-hidden="true" />
          Active sessions and provider status below always reflect the server's connected profile — switching
          this override does not request a different profile's live session stream (no WS message for that
          exists yet).
        </p>
      </section>

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">Providers {serverVersion && <span className="text-muted font-normal">· server v{serverVersion}</span>}</h3>
        {providers.length === 0 ? (
          <EmptyState icon={Info} title="No providers registered" />
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {providers.map((provider) => (
              <ProviderStatusCard
                key={provider.instanceId}
                provider={provider}
                status={providerStatuses[provider.instanceId]}
                onClick={() => handleRefreshProvider(provider.instanceId)}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
