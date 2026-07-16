import { useEffect, useState } from 'react';
import { Sun, Moon, Info, ExternalLink, Save, Loader2 } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';
import { ProfileEditor } from '../components/ProfileEditor.js';
import type { WakeAutonomyValue, ConfigProfileSummary, RoutingCandidateSummary } from '@git-agent-harness/contracts';

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
  const profileConfig = useGahStore((s) => s.profileConfig);
  const fetchConfig = useGahStore((s) => s.fetchConfig);
  const fetchProfileConfig = useGahStore((s) => s.fetchProfileConfig);
  const setConfig = useGahStore((s) => s.setConfig);
  const clearConfigErrors = useGahStore((s) => s.clearConfigErrors);
  const configuredProfiles = profiles.data ?? [];
  const selectedName = profileOverride ?? profile ?? '';
  const selected = configuredProfiles.find((p) => p.name === selectedName);

  useEffect(() => {
    fetchProfiles();
    fetchConfig();
  }, [fetchProfiles, fetchConfig]);

  useEffect(() => {
    if (selectedName) {
      fetchProfileConfig(selectedName);
    }
  }, [selectedName, fetchProfileConfig]);

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

      <ProfileConfigViewerSection
        selectedName={selectedName}
        profileConfig={profileConfig}
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

  const validationTimeoutValue = validationTimeout.trim();
  const parsedValidationTimeout = Number(validationTimeoutValue);
  const validationTimeoutError = validationTimeoutValue !== ''
    && (!Number.isInteger(parsedValidationTimeout) || parsedValidationTimeout < 1)
    ? 'Validation timeout must be a whole number of seconds greater than zero.'
    : null;

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
    if (validationTimeoutError) return;
    const parsed = parallel.trim() === '' ? undefined : Math.max(1, parseInt(parallel, 10) || 1);
    const hasValidationTimeout = validationTimeoutValue !== '';
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
            aria-invalid={validationTimeoutError != null}
            aria-describedby={validationTimeoutError ? 'validation-timeout-error' : undefined}
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
          />
          {validationTimeoutError && (
            <p id="validation-timeout-error" role="alert" className="text-xs text-critical mt-1">
              {validationTimeoutError}
            </p>
          )}
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
        disabled={saving || validationTimeoutError != null}
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

interface ProfileConfigViewerSectionProps {
  selectedName: string;
  profileConfig: {
    data: ConfigProfileSummary | null;
    loading: boolean;
    error: string | null;
  };
}

function ProfileConfigViewerSection({ selectedName, profileConfig }: ProfileConfigViewerSectionProps) {
  if (!selectedName) {
    return (
      <section className="card-padded max-w-3xl">
        <h3 className="text-sm font-semibold text-primary mb-1">Effective profile configuration</h3>
        <p className="text-xs text-muted">Select a profile to view effective routing, review chain, and context budget configuration.</p>
      </section>
    );
  }

  if (profileConfig.loading && !profileConfig.data) {
    return (
      <section className="card-padded max-w-3xl">
        <h3 className="text-sm font-semibold text-primary mb-3">Effective profile configuration</h3>
        <p className="text-xs text-muted">Loading profile configuration…</p>
      </section>
    );
  }

  if (profileConfig.error && !profileConfig.data) {
    return (
      <section className="card-padded max-w-3xl">
        <h3 className="text-sm font-semibold text-primary mb-3">Effective profile configuration</h3>
        <p className="text-xs text-critical">Failed to load effective config: {profileConfig.error}</p>
      </section>
    );
  }

  const effective = profileConfig.data;
  if (!effective) {
    return null;
  }

  return (
    <section className="card-padded max-w-3xl">
      <h3 className="text-sm font-semibold text-primary mb-1">Effective profile configuration</h3>
      <p className="text-xs text-muted mb-3">
        Read-only effective routing and policy for <span className="font-mono text-secondary">{selectedName}</span>.
      </p>

      {profileConfig.error && (
        <p className="text-xs text-critical mb-2">Last refresh error: {profileConfig.error}</p>
      )}

      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <div className="card-padded border border-subtle">
          <h4 className="text-xs font-semibold text-primary mb-2">Policy</h4>
          <p className="text-xs">
            Merge policy: <span className="font-mono text-secondary">{effective.merge_policy}</span>
          </p>
          <p className="text-xs text-muted mt-1">Profile: {effective.profile}</p>
          <div className="mt-2 text-xs text-muted">
            <p>Max repair cycles per ticket: {effective.max_fix_attempts_per_mr}</p>
            <p>Max implementation failures per ticket: {effective.max_implementation_failures_per_ticket}</p>
            <p>Max review cycles per ticket: {effective.max_review_cycles_per_ticket}</p>
            <p>Max paid reviews per ticket: {effective.max_paid_reviews_per_ticket}</p>
          </div>
        </div>

        <div className="card-padded border border-subtle">
          <h4 className="text-xs font-semibold text-primary mb-2">Review escalation</h4>
          <p className="text-xs">
            Routine reviewer:{' '}
            {effective.routine_reviewer ? formatCandidateLabel(effective.routine_reviewer) : 'None configured'}
          </p>
          <p className="text-xs text-muted mt-1">Escalation chain:</p>
          {effective.escalatory_reviewers.length === 0 ? (
            <p className="text-xs text-muted">No configured escalation chain.</p>
          ) : (
            <ul className="text-xs text-secondary">
              {effective.escalatory_reviewers.map((candidate, index) => (
                <li key={`${candidate.backend}-${index}`} className="mt-1">
                  {index + 1}. {formatCandidateLabel(candidate)}
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      <div className="mt-3 grid grid-cols-1 md:grid-cols-2 gap-3">
        <CandidateTable title="PM candidates" candidates={effective.pm_candidates} />
        <CandidateTable title="Improve candidates" candidates={effective.improve_candidates} />
        <CandidateTable title="Review candidates" candidates={effective.review_candidates} />
      </div>

      <div className="mt-3 card-padded border border-subtle">
        <h4 className="text-xs font-semibold text-primary mb-2">Context budgets</h4>
        <p className="text-xs text-muted">
          Effective context: soft limit {effective.context.effective.soft_limit_tokens} · hard limit{' '}
          {effective.context.effective.hard_limit_tokens}
        </p>
        <p className="text-xs text-muted mt-1">
          Default context: soft limit {effective.context.global.soft_limit_tokens} · hard limit{' '}
          {effective.context.global.hard_limit_tokens}
        </p>
        <p className="text-xs text-muted mt-1">
          Fresh context on review/fix: {effective.context.effective.fresh_context_on_review ? 'yes' : 'no'} /{' '}
          {effective.context.effective.fresh_context_on_fix ? 'yes' : 'no'}
        </p>
        <p className="text-xs text-muted mt-1">
          Include full Git history: {effective.context.effective.include_full_git_history ? 'yes' : 'no'} · Transcript in
          review: {effective.context.effective.include_full_worker_transcript_in_review ? 'yes' : 'no'}
        </p>
        {effective.context.profile_override && (
          <p className="text-xs text-muted mt-1">Profile context override is present.</p>
        )}
      </div>
    </section>
  );
}

function CandidateTable({ title, candidates }: { title: string; candidates: RoutingCandidateSummary[] }) {
  if (candidates.length === 0) {
    return (
      <div className="card-padded border border-subtle">
        <h4 className="text-xs font-semibold text-primary mb-2">{title}</h4>
        <p className="text-xs text-muted">No candidates configured.</p>
      </div>
    );
  }

  return (
    <div className="card-padded border border-subtle">
      <h4 className="text-xs font-semibold text-primary mb-2">{title}</h4>
      <div className="space-y-1.5">
        {candidates.map((candidate, index) => (
          <div key={`${candidate.backend}-${candidate.model ?? 'none'}-${index}`} className="text-xs">
            <span className="text-secondary">{formatCandidateLabel(candidate)}</span>
            <span className="text-muted"> · priority {candidate.priority}</span>
            {candidate.requires_approval ? <span className="text-warning"> · requires approval</span> : null}
            {candidate.quota_pool ? <span className="text-muted"> · pool {candidate.quota_pool}</span> : null}
          </div>
        ))}
      </div>
    </div>
  );
}

function formatCandidateLabel(candidate: RoutingCandidateSummary): string {
  return candidate.model ? `${candidate.backend}/${candidate.model}` : candidate.backend;
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
