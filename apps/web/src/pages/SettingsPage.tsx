import { useEffect } from 'react';
import { Sun, Moon, Info, ExternalLink } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';

const SCM_PROVIDER_KINDS = new Set(['github', 'gitlab']);

export function SettingsPage() {
  const { providers, providerStatuses, sendMessage, isConnected, serverVersion, profile } = useWebSocket();
  const { theme, setTheme, profileOverride, setProfileOverride } = useUiStore();
  const profiles = useGahStore((s) => s.profiles);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);

  useEffect(() => {
    fetchProfiles();
  }, [fetchProfiles]);

  const configuredProfiles = profiles.data ?? [];
  const selectedName = profileOverride ?? profile ?? '';
  const selected = configuredProfiles.find((p) => p.name === selectedName);

  const agentBackends = providers.filter((p) => !SCM_PROVIDER_KINDS.has(p.providerKind));
  const activeScmProvider = selected?.provider
    ? providers.find((p) => p.providerKind === selected.provider)
    : null;

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
          Which configured GAH repo Overview/Work/Telemetry/Quota/Events read from.
        </p>
        {profiles.loading && !profiles.data ? (
          <p className="text-xs text-muted">Loading configured profiles…</p>
        ) : profiles.error ? (
          <p className="text-xs text-critical">Failed to load profiles: {profiles.error}</p>
        ) : configuredProfiles.length === 0 ? (
          <p className="text-xs text-muted">No profiles found in the GAH config.</p>
        ) : (
          <select
            value={selectedName}
            onChange={(e) => setProfileOverride(e.target.value || null)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
          >
            {configuredProfiles.map((p) => (
              <option key={p.name} value={p.name}>
                {p.display_name} ({p.name})
              </option>
            ))}
          </select>
        )}
        {selected?.web_url && (
          <a
            href={selected.web_url}
            target="_blank"
            rel="noopener noreferrer"
            className="mt-2 inline-flex items-center gap-1 text-xs text-accent hover:underline"
          >
            <ExternalLink size={12} aria-hidden="true" />
            {selected.repo}
          </a>
        )}
        {activeScmProvider && (
          <div className="mt-3 pt-3 border-t border-subtle">
            <ProviderStatusCard
              provider={activeScmProvider}
              status={providerStatuses[activeScmProvider.instanceId]}
              onClick={() => handleRefreshProvider(activeScmProvider.instanceId)}
            />
          </div>
        )}
        <p className="mt-3 text-xs text-muted inline-flex items-start gap-1.5">
          <Info size={13} className="shrink-0 mt-0.5" aria-hidden="true" />
          Switching profiles reconnects the live WebSocket feed as well as refreshing the REST-backed pages.
        </p>
      </section>

      <section>
        <h3 className="text-sm font-semibold text-primary mb-3">Agent backends {serverVersion && <span className="text-muted font-normal">· server v{serverVersion}</span>}</h3>
        {agentBackends.length === 0 ? (
          <EmptyState icon={Info} title="No agent backends registered" />
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {agentBackends.map((provider) => (
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
