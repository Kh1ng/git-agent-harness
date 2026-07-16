import { useEffect, useState } from 'react';
import { Sun, Moon, Info, ExternalLink, Save, Loader2 } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';
import { ProfileEditor } from '../components/ProfileEditor.js';
import type { WakeAutonomyValue } from '@git-agent-harness/contracts';

const SCM_PROVIDER_KINDS = new Set(['github', 'gitlab']);
const WAKE_AUTONOMY_OPTIONS: { value: WakeAutonomyValue; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'review_only', label: 'Review only' },
  { value: 'full', label: 'Full' },
];

export function SettingsPage() {
  const { providers, providerStatuses, backendConfigured, sendMessage, isConnected, serverVersion, profile } = useWebSocket();
  const { theme, setTheme, profileOverride, setProfileOverride } = useUiStore();
  const profiles = useGahStore((s) => s.profiles);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  const config = useGahStore((s) => s.config);
  const fetchConfig = useGahStore((s) => s.fetchConfig);
  const setConfig = useGahStore((s) => s.setConfig);
  const clearConfigErrors = useGahStore((s) => s.clearConfigErrors);

  useEffect(() => {
    fetchProfiles();
    fetchConfig();
  }, [fetchProfiles, fetchConfig]);

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
      <PageHeader title="Settings" description="Appearance, profile, dispatch, and manager configuration" />

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

      <DispatchSettingsSection
        selectedName={selectedName}
        selected={selected}
        profileLoading={profiles.loading}
        profileError={profiles.error}
      />

      <GlobalManagerSection
        config={config}
        setConfig={setConfig}
        clearConfigErrors={clearConfigErrors}
      />

      <section>
        <ProfileEditor />
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

interface DispatchSettingsSectionProps {
  selectedName: string;
  selected?: {
    max_parallel_workers: number | null;
    validation_timeout_seconds?: number | null;
    manager_wake_autonomy: WakeAutonomyValue | null;
  };
  profileLoading: boolean;
  profileError: string | null;
}

function DispatchSettingsSection({
  selectedName,
  selected,
  profileLoading,
  profileError,
}: DispatchSettingsSectionProps) {
  const updateProfile = useGahStore((s) => s.updateProfile);
  const profileCrud = useGahStore((s) => s.profileCrud);

  const [parallel, setParallel] = useState<string>('');
  const [validationTimeout, setValidationTimeout] = useState<string>('');
  const [autonomy, setAutonomy] = useState<WakeAutonomyValue>('off');

  // Re-seed the form whenever the selected profile changes or its values load.
  useEffect(() => {
    setParallel(selected?.max_parallel_workers != null ? String(selected.max_parallel_workers) : '');
    setValidationTimeout(selected?.validation_timeout_seconds != null ? String(selected.validation_timeout_seconds) : '');
    setAutonomy(selected?.manager_wake_autonomy ?? 'off');
  }, [selectedName, selected?.max_parallel_workers, selected?.validation_timeout_seconds, selected?.manager_wake_autonomy]);

  if (profileLoading && !selected) {
    return (
      <section className="card-padded max-w-md">
        <h3 className="text-sm font-semibold text-primary mb-3">Dispatch settings</h3>
        <p className="text-xs text-muted">Loading profiles…</p>
      </section>
    );
  }

  if (profileError || !selected) {
    return (
      <section className="card-padded max-w-md">
        <h3 className="text-sm font-semibold text-primary mb-3">Dispatch settings</h3>
        <p className="text-xs text-muted">
          {profileError ? `Failed to load profiles: ${profileError}` : 'Select a profile to edit its dispatch settings.'}
        </p>
      </section>
    );
  }

  const handleSave = async () => {
    const parsed = parallel.trim() === '' ? undefined : Math.max(1, parseInt(parallel, 10) || 1);
    const hasValidationTimeout = validationTimeout.trim() !== '';
    const parsedValidationTimeout = hasValidationTimeout ? Math.max(1, parseInt(validationTimeout, 10) || 1) : undefined;
    await updateProfile(selectedName, {
      max_parallel_workers: parsed,
      manager_wake_autonomy: autonomy,
      ...(hasValidationTimeout
        ? { validation_timeout_seconds: parsedValidationTimeout }
        : { clear: ['validation_timeout_seconds'] }),
    });
  };

  const saveError = profileCrud.updateError;
  const saving = profileCrud.updating;

  return (
    <section className="card-padded max-w-md">
      <h3 className="text-sm font-semibold text-primary mb-1">Dispatch settings</h3>
      <p className="text-xs text-muted mb-3">
        Per-profile loop behavior for <span className="font-mono text-secondary">{selectedName}</span>.
        Changes apply on the next loop iteration — no restart needed.
      </p>

      <div className="space-y-3">
        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Max parallel workers
          </label>
          <input
            type="number"
            min={1}
            value={parallel}
            onChange={(e) => setParallel(e.target.value)}
            placeholder="1"
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
          />
          <p className="text-xs text-muted mt-1">
            How many tickets <code>gah loop</code> may execute concurrently (default 1).
          </p>
        </div>

        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Validation command timeout (seconds)
          </label>
          <input
            type="number"
            min={1}
            value={validationTimeout}
            onChange={(e) => setValidationTimeout(e.target.value)}
            placeholder="300"
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
          />
          <p className="text-xs text-muted mt-1">
            Per-profile timeout for <code>validation_commands</code>.
            This is separate from backend idle timeouts such as <code>codex_idle_timeout_seconds</code> and
            <code>claude_idle_timeout_seconds</code>.
          </p>
        </div>

        <div>
          <label className="block text-xs font-medium text-secondary mb-1">
            Manager wake autonomy
          </label>
          <select
            value={autonomy}
            onChange={(e) => setAutonomy(e.target.value as WakeAutonomyValue)}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
          >
            {WAKE_AUTONOMY_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>
          <p className="text-xs text-muted mt-1">
            What a woken manager agent may do when a notify-worthy event fires.
          </p>
        </div>
      </div>

      {saveError && (
        <p className="mt-3 text-xs text-critical">Failed to save: {saveError}</p>
      )}
      {profileCrud.lastUpdateSuccess && !saveError && (
        <p className="mt-3 text-xs text-green-600">Dispatch settings saved.</p>
      )}

      <button
        onClick={handleSave}
        disabled={saving}
        className="mt-3 inline-flex items-center gap-1.5 px-3 py-1.5 bg-accent text-white rounded-md text-sm font-medium hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        {saving ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <Save size={14} aria-hidden="true" />}
        {saving ? 'Saving…' : 'Save dispatch settings'}
      </button>
    </section>
  );
}

interface GlobalManagerSectionProps {
  config: { data: { current_manager: string | null } | null; loading: boolean; error: string | null };
  setConfig: (data: { current_manager?: string | null; clear?: string[] }) => Promise<void>;
  clearConfigErrors: () => void;
}

function GlobalManagerSection({ config, setConfig, clearConfigErrors }: GlobalManagerSectionProps) {
  const [manager, setManager] = useState<string>('');

  useEffect(() => {
    setManager(config.data?.current_manager ?? '');
  }, [config.data?.current_manager]);

  const handleSave = async () => {
    const value = manager.trim();
    await setConfig(value === '' ? { clear: ['current_manager'] } : { current_manager: value });
  };

  return (
    <section className="card-padded max-w-md">
      <h3 className="text-sm font-semibold text-primary mb-1">Global manager</h3>
      <p className="text-xs text-muted mb-3">
        Which agent CLI is currently on call as the operator's manager across all
        profiles/projects (the manager-wake "who's on call"). Global, not per-profile.
      </p>

      <label className="block text-xs font-medium text-secondary mb-1">
        Current manager
      </label>
      <input
        type="text"
        value={manager}
        onChange={(e) => setManager(e.target.value)}
        placeholder="e.g. claude, codex, hermes"
        className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
      />
      <p className="text-xs text-muted mt-1">
        Leave blank and save to clear it. Changes apply to the next loop iteration without a restart.
      </p>

      {config.error && (
        <p className="mt-3 text-xs text-critical">Error: {config.error}</p>
      )}

      <button
        onClick={handleSave}
        disabled={config.loading}
        className="mt-3 inline-flex items-center gap-1.5 px-3 py-1.5 bg-accent text-white rounded-md text-sm font-medium hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        {config.loading ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <Save size={14} aria-hidden="true" />}
        {config.loading ? 'Saving…' : 'Save global manager'}
      </button>
    </section>
  );
}
