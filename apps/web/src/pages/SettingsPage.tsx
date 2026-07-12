import { useEffect, useState } from 'react';
import { Sun, Moon, Info, ExternalLink } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { gahApi } from '../api/client.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';
import { ProfileEditor } from '../components/ProfileEditor.js';

const SCM_PROVIDER_KINDS = new Set(['github', 'gitlab']);

export function SettingsPage() {
  const { providers, providerStatuses, backendConfigured, sendMessage, isConnected, serverVersion, profile } = useWebSocket();
  const { theme, setTheme, profileOverride, setProfileOverride } = useUiStore();
  const profiles = useGahStore((s) => s.profiles);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  
  const [settings, setSettings] = useState<{
    max_parallel_workers: number;
    current_manager: string | null;
    manager_wake_autonomy: string | null;
  }>({
    max_parallel_workers: 1,
    current_manager: null,
    manager_wake_autonomy: null
  });
  const [loadingSettings, setLoadingSettings] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    fetchProfiles();
  }, [fetchProfiles]);

  useEffect(() => {
    const loadSettings = async () => {
      try {
        setLoadingSettings(true);
        const loadedSettings = await gahApi.getSettings();
        setSettings(loadedSettings);
        setError(null);
      } catch (err) {
        console.error('Failed to load settings:', err);
        setError(err instanceof Error ? err.message : 'Failed to load settings');
      } finally {
        setLoadingSettings(false);
      }
    };
    loadSettings();
  }, []);

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
      </section>

      <section>
        <ProfileEditor />
      </section>

      <section className="card-padded max-w-md">
        <h3 className="text-sm font-semibold text-primary mb-3">Controller Settings</h3>
        <p className="text-xs text-muted mb-4">
          Configure global controller behavior and parallelism
        </p>
        
        {error && (
          <div className="mb-4 p-3 bg-critical/10 border border-critical/20 rounded-md">
            <p className="text-sm text-critical">Error loading settings: {error}</p>
          </div>
        )}
        
        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-primary mb-1">
              Max Parallel Workers
            </label>
            <input
              type="number"
              min="1"
              max="10"
              className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
              value={settings.max_parallel_workers || 1}
              onChange={async (e) => {
                const value = parseInt(e.target.value) || 1;
                try {
                  await gahApi.updateSettings({ max_parallel_workers: value });
                  setSettings(prev => ({ ...prev, max_parallel_workers: value }));
                } catch (err) {
                  console.error('Failed to update max_parallel_workers:', err);
                }
              }}
              disabled={loadingSettings}
            />
            <p className="text-xs text-muted mt-1">
              How many tickets may execute concurrently (default: 1)
            </p>
          </div>
          
          <div>
            <label className="block text-sm font-medium text-primary mb-1">
              Current Manager
            </label>
            <select
              className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
              value={settings.current_manager || ''}
              onChange={async (e) => {
                const value = e.target.value === '' ? null : e.target.value;
                try {
                  await gahApi.updateSettings({ current_manager: value });
                  setSettings(prev => ({ ...prev, current_manager: value }));
                } catch (err) {
                  console.error('Failed to update current_manager:', err);
                }
              }}
              disabled={loadingSettings}
            >
              <option value="">None (disabled)</option>
              <option value="claude">Claude</option>
              <option value="codex">Codex</option>
              <option value="vibe">Vibe</option>
              <option value="agy">AGY</option>
            </select>
            <p className="text-xs text-muted mt-1">
              Which agent CLI is currently acting as manager
            </p>
          </div>
          
          <div>
            <label className="block text-sm font-medium text-primary mb-1">
              Manager Wake Autonomy
            </label>
            <select
              className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
              value={settings.manager_wake_autonomy || 'off'}
              onChange={async (e) => {
                const value = e.target.value === 'off' ? null : e.target.value;
                try {
                  await gahApi.updateSettings({ manager_wake_autonomy: value });
                  setSettings(prev => ({ ...prev, manager_wake_autonomy: value }));
                } catch (err) {
                  console.error('Failed to update manager_wake_autonomy:', err);
                }
              }}
              disabled={loadingSettings}
            >
              <option value="off">Off</option>
              <option value="review_only">Review Only</option>
              <option value="full">Full</option>
            </select>
            <p className="text-xs text-muted mt-1">
              How much autonomy the woken manager agent has
            </p>
          </div>
        </div>
        
        {loadingSettings && (
          <div className="mt-4 p-3 bg-subtle/10 border border-subtle/20 rounded-md">
            <p className="text-sm text-muted">Loading settings...</p>
          </div>
        )}
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
                backendConfigured={backendConfigured}
                onClick={() => handleRefreshProvider(provider.instanceId)}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
