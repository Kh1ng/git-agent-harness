import { useEffect, useState } from 'react';
import { Sun, Moon, Info, ExternalLink, Save, Loader2, RefreshCw, Eye, EyeOff, Copy, Check, ChevronRight } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';
import { ProfileEditor } from '../components/ProfileEditor.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { oldestFetchedAt } from '../lib/format.js';
import { gahApi, GahApiError } from '../api/client.js';
import type { WakeAutonomyValue, SettingsConfigProfileSummary, RoutingCandidateSummary, ManagerChatSettingsSummary, ProfileSummary, GatewaySettingsSummary, MemoryContextPolicy, SkillSummary, AdminUpdatePendingInfo, AdminUpdateState } from '@git-agent-harness/contracts';

const SCM_PROVIDER_KINDS = new Set(['github', 'gitlab']);
const SETTINGS_REFRESH_MS = 60 * 1000;
const SETTINGS_SECTIONS_KEY = 'gah.settings.openSections';
type SettingsSectionId = 'general' | 'skills' | 'memory' | 'factory';
const SETTINGS_SECTION_IDS: SettingsSectionId[] = ['general', 'skills', 'memory', 'factory'];
const WAKE_AUTONOMY_OPTIONS: { value: WakeAutonomyValue; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'review_only', label: 'Review only' },
  { value: 'full', label: 'Full' },
];

export function SettingsPage() {
  const { providers, providerStatuses, sendMessage, isConnected, serverVersion, profile } = useWebSocket();
  const { theme, setTheme, profileOverride, setProfileOverride } = useUiStore();
  const profiles = useGahStore((s) => s.profiles);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  const config = useGahStore((s) => s.config);
  const profileConfig = useGahStore((s) => s.profileConfig);
  const fetchConfig = useGahStore((s) => s.fetchConfig);
  const fetchProfileConfig = useGahStore((s) => s.fetchProfileConfig);
  const setConfig = useGahStore((s) => s.setConfig);
  const clearConfigErrors = useGahStore((s) => s.clearConfigErrors);
  const doctor = useGahStore((s) => s.doctor);
  const fetchDoctor = useGahStore((s) => s.fetchDoctor);
  const configuredProfiles = profiles.data ?? [];
  const selectedName = profileOverride ?? profile ?? '';
  const selected = configuredProfiles.find((p) => p.name === selectedName);
  const [openSections, setOpenSections] = useState<Set<SettingsSectionId>>(() => {
    try {
      const raw = window.localStorage.getItem(SETTINGS_SECTIONS_KEY);
      if (raw === null) return new Set(['general']);
      const stored = JSON.parse(raw);
      const valid = Array.isArray(stored)
        ? stored.filter((id): id is SettingsSectionId => SETTINGS_SECTION_IDS.includes(id as SettingsSectionId))
        : [];
      return new Set<SettingsSectionId>(valid.slice(0, 1));
    } catch {
      return new Set(['general']);
    }
  });

  useEffect(() => {
    fetchProfiles();
    fetchConfig();
  }, [fetchProfiles, fetchConfig]);

  useEffect(() => {
    if (selectedName) {
      fetchProfileConfig(selectedName);
      fetchDoctor(selectedName);
    }
  }, [selectedName, fetchProfileConfig, fetchDoctor]);

  const refreshAll = () => {
    fetchProfiles({ force: true });
    fetchConfig({ force: true });
    if (selectedName) {
      fetchProfileConfig(selectedName, { force: true });
      fetchDoctor(selectedName, { force: true });
    }
  };
  useAutoRefresh(refreshAll, SETTINGS_REFRESH_MS);
  useWsReconnectRefresh(refreshAll);
  const lastUpdated = oldestFetchedAt(profiles.fetchedAt, config.fetchedAt, profileConfig.fetchedAt, doctor.fetchedAt);

  const agentBackends = providers.filter((p) => !SCM_PROVIDER_KINDS.has(p.providerKind));
  const activeScmProvider = selected?.provider
    ? providers.find((p) => p.providerKind === selected.provider)
    : null;

  const handleRefreshProvider = (instanceId: string) => {
    if (isConnected) {
      sendMessage({ type: 'provider.refresh', requestId: `req_${Date.now()}`, instanceId });
    }
  };

  const setSectionOpen = (id: SettingsSectionId, open: boolean) => {
    setOpenSections(() => {
      const next = new Set<SettingsSectionId>(open ? [id] : []);
      try {
        window.localStorage.setItem(SETTINGS_SECTIONS_KEY, JSON.stringify([...next]));
      } catch {
        // Storage unavailable: disclosure state simply does not survive reload.
      }
      return next;
    });
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title="Settings"
        description="Profiles, backends, skills, memory, and node operation"
        onRefresh={refreshAll}
        refreshing={profiles.loading || config.loading}
        lastUpdated={lastUpdated}
      />

      <section className="card-padded max-w-4xl">
        <h3 className="text-sm font-semibold text-primary mb-2">Profile context</h3>
        <p className="text-xs text-muted mb-3">
          Which configured GAH repo Overview/Work/Telemetry/Quota/Events and these settings read from.
        </p>
        <div className="max-w-md">
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
        </div>
      </section>

      <div className="max-w-4xl space-y-3">
        <div className="grid gap-3 sm:grid-cols-2">
          <SettingsSectionButton id="general" title="General" description="Appearance, chat, updates, and backends." open={openSections.has('general')} onToggle={setSectionOpen} />
          <SettingsSectionButton id="skills" title="Skill bank" description="Versioned skills available to backends." open={openSections.has('skills')} onToggle={setSectionOpen} />
          <SettingsSectionButton id="memory" title="TDAI / memory" description="Gateway health, credentials, and recall policy." open={openSections.has('memory')} onToggle={setSectionOpen} />
          <SettingsSectionButton id="factory" title="Factory / profile management" description="Dispatch, effective config, and profiles." open={openSections.has('factory')} onToggle={setSectionOpen} />
        </div>

        {openSections.has('general') && <SettingsSectionPanel id="general">
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

      <section className="card-padded max-w-3xl">
        <div className="flex items-start justify-between gap-3 mb-3">
          <div>
            <h3 className="text-sm font-semibold text-primary">Node readiness</h3>
            <p className="text-xs text-muted mt-1">
              On-demand config, provider authentication, filesystem, and backend executable checks.
            </p>
          </div>
          <button
            type="button"
            onClick={() => fetchDoctor(selectedName || undefined, { force: true })}
            disabled={!selectedName || doctor.loading}
            className="inline-flex items-center gap-1.5 px-2.5 py-1.5 border border-subtle rounded-md text-xs text-secondary hover:text-primary disabled:opacity-50"
          >
            <RefreshCw size={13} className={doctor.loading ? 'animate-spin' : ''} aria-hidden="true" />
            Check now
          </button>
        </div>
        {!selectedName ? (
          <p className="text-xs text-muted">Select a profile to check this node.</p>
        ) : doctor.loading && !doctor.data ? (
          <p className="text-xs text-muted">Running readiness checks…</p>
        ) : doctor.error ? (
          <p className="text-xs text-critical">Readiness check failed to run: {doctor.error}</p>
        ) : doctor.data ? (
          <>
            <div className="flex items-center gap-2 mb-3 text-xs text-muted">
              <StatusBadge
                tone={doctor.data.overall_status === 'ok' ? 'good' : doctor.data.overall_status === 'warn' ? 'serious' : 'critical'}
                label={doctor.data.overall_status}
              />
              <span>{doctor.data.checks.length} checks</span>
            </div>
            <div className="max-h-96 overflow-auto divide-y divide-subtle border border-subtle rounded-md">
              {doctor.data.checks.map((check, index) => (
                <div key={`${check.profile ?? 'node'}-${check.name}-${index}`} className="p-2.5 flex items-start justify-between gap-3 text-xs">
                  <div className="min-w-0">
                    <p className="text-primary">{check.name}</p>
                    <p className="text-muted mt-0.5 break-words">{check.detail}</p>
                  </div>
                  <StatusBadge
                    tone={check.status === 'ok' ? 'good' : check.status === 'warn' ? 'serious' : 'critical'}
                    label={check.status}
                  />
                </div>
              ))}
            </div>
          </>
        ) : (
          <p className="text-xs text-muted">No readiness result yet.</p>
        )}
      </section>

      <GlobalManagerSection
        config={config}
        setConfig={setConfig}
        clearConfigErrors={clearConfigErrors}
      />

      <ManagerChatSettingsSection configuredProfiles={configuredProfiles} />
      <AdminUpdateSection />
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
        </SettingsSectionPanel>}

        {openSections.has('skills') && <SettingsSectionPanel id="skills">
          <SkillBankSettingsSection />
        </SettingsSectionPanel>}

        {openSections.has('memory') && <SettingsSectionPanel id="memory">
          <GatewaySettingsSection configuredProfiles={configuredProfiles} />
          <AddNodeSection />
        </SettingsSectionPanel>}

        {openSections.has('factory') && <SettingsSectionPanel id="factory">
          <DispatchSettingsSection
            selectedName={selectedName}
            selected={selected}
            profileLoading={profiles.loading}
            profileError={profiles.error}
          />
          <ProfileConfigViewerSection
            selectedName={selectedName}
            profileConfig={profileConfig}
          />
          <section>
            <ProfileEditor />
          </section>
        </SettingsSectionPanel>}
      </div>
    </div>
  );
}

interface SettingsSectionButtonProps {
  id: SettingsSectionId;
  title: string;
  description: string;
  open: boolean;
  onToggle: (id: SettingsSectionId, open: boolean) => void;
}

function SettingsSectionButton({ id, title, description, open, onToggle }: SettingsSectionButtonProps) {
  return (
    <button
      id={`settings-${id}-button`}
      type="button"
      aria-expanded={open}
      aria-controls={`settings-${id}-panel`}
      onClick={() => onToggle(id, !open)}
      className={`card flex min-h-20 items-center gap-3 px-4 py-3.5 text-left transition-colors sm:px-5 ${open ? 'border-accent bg-accent/5' : 'hover:border-accent/50'}`}
    >
      <ChevronRight size={17} className={`shrink-0 text-muted transition-transform ${open ? 'rotate-90' : ''}`} aria-hidden="true" />
      <span className="min-w-0">
        <span className="block text-sm font-semibold text-primary">{title}</span>
        <span className="block text-xs text-muted mt-0.5">{description}</span>
      </span>
    </button>
  );
}

function SettingsSectionPanel({ id, children }: { id: SettingsSectionId; children: React.ReactNode }) {
  return (
    <div
      id={`settings-${id}-panel`}
      role="region"
      aria-labelledby={`settings-${id}-button`}
      className="card space-y-6 p-4 sm:p-5"
    >
      {children}
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
    data: SettingsConfigProfileSummary | null;
    loading: boolean;
    error: string | null;
  };
}

export function ProfileConfigViewerSection({ selectedName, profileConfig }: ProfileConfigViewerSectionProps) {
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
        <h4 className="text-xs font-semibold text-primary mb-2">Task routing rules</h4>
        {effective.task_routing_rules.length === 0 ? (
          <p className="text-xs text-muted">No class-specific routing rules configured.</p>
        ) : (
          <ol className="space-y-2 text-xs text-secondary">
            {effective.task_routing_rules.map((rule, index) => (
              <li key={`task-rule-${index}`}>
                <span className="font-semibold">{index + 1}.</span>{' '}
                modes {formatList(rule.modes)} · classes {formatList(rule.task_classes)} · difficulty{' '}
                {formatList(rule.difficulties)} · risk {formatList(rule.risks)}
                <div className="text-muted ml-4">
                  {rule.candidates.map(formatCandidateLabel).join(' → ') || 'No candidates configured'}
                </div>
              </li>
            ))}
          </ol>
        )}
      </div>

      <div className="mt-3 card-padded border border-subtle">
        <h4 className="text-xs font-semibold text-primary mb-2">Context budgets</h4>
        <p className="text-xs text-muted">
          Default context: soft limit {effective.context.global.soft_limit_tokens} · hard limit{' '}
          {effective.context.global.hard_limit_tokens}
        </p>
        {effective.context.profile_override && (
          <p className="text-xs text-muted mt-1">Profile context override is present.</p>
        )}
        <p className="text-xs text-muted mt-2">
          Effective budgets differ per routed backend when a `context.backends.&lt;name&gt;` override applies:
        </p>
        {effective.context.effective_by_backend.length === 0 ? (
          <p className="text-xs text-muted mt-1">No backends are routed for this profile.</p>
        ) : (
          <ul className="text-xs text-secondary mt-1">
            {effective.context.effective_by_backend.map((entry) => (
              <li key={entry.backend} className="mt-1">
                <span className="font-mono">{entry.backend}</span>: soft limit {entry.effective.soft_limit_tokens} · hard
                limit {entry.effective.hard_limit_tokens} · fresh on review/fix:{' '}
                {entry.effective.fresh_context_on_review ? 'yes' : 'no'}/{entry.effective.fresh_context_on_fix ? 'yes' : 'no'}
                {entry.backend_override && <span className="text-muted"> (backend override applied)</span>}
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="mt-3 card-padded border border-subtle">
        <h4 className="text-xs font-semibold text-primary mb-2">Notifications</h4>
        <p className="text-xs text-secondary">
          Target: {effective.notifications.configured ? effective.notifications.transport ?? 'unknown' : 'not configured'}
        </p>
        <p className="text-xs text-muted mt-1">
          Manager wake: {effective.notifications.manager_wake_autonomy} · dev env:{' '}
          {effective.notifications.env_file_configured ? 'configured' : 'not configured'} · prod env:{' '}
          {effective.notifications.env_file_prod_configured ? 'configured' : 'not configured'}
        </p>
        <p className="text-xs text-muted mt-1">Command contents and credentials are intentionally excluded.</p>
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
            <span className="text-muted"> · pool {candidate.quota_pool ?? 'unknown'}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function formatCandidateLabel(candidate: RoutingCandidateSummary): string {
  return `${candidate.backend}/${candidate.model ?? 'unknown'}`;
}

function formatList(values: string[]): string {
  return values.length > 0 ? values.join(', ') : 'any';
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

function ManagerChatSettingsSection({ configuredProfiles }: { configuredProfiles: ProfileSummary[] }) {
  const [settings, setSettings] = useState<ManagerChatSettingsSummary | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [newOverrideProfile, setNewOverrideProfile] = useState('');
  const [newOverrideBackend, setNewOverrideBackend] = useState('');

  const load = () => {
    gahApi
      .getManagerChatSettings()
      .then((data) => {
        setSettings(data);
        setError(null);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  };

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const save = async (update: { defaultBackend?: string; profileOverrides?: Record<string, string> }) => {
    setLoading(true);
    try {
      await gahApi.setManagerChatSettings(update);
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  const removeOverride = (profile: string) => {
    if (!settings) return;
    const next = { ...settings.profileOverrides };
    delete next[profile];
    save({ profileOverrides: next });
  };

  const addOverride = () => {
    if (!settings || !newOverrideProfile || !newOverrideBackend) return;
    save({ profileOverrides: { ...settings.profileOverrides, [newOverrideProfile]: newOverrideBackend } });
    setNewOverrideProfile('');
    setNewOverrideBackend('');
  };

  if (!settings) {
    return (
      <section className="card-padded max-w-md">
        <h3 className="text-sm font-semibold text-primary mb-1">Chat</h3>
        {error ? <p className="text-xs text-critical">Failed to load: {error}</p> : <p className="text-xs text-muted">Loading…</p>}
      </section>
    );
  }

  const backendOptions = settings.availableBackends;
  const overrideEntries = Object.entries(settings.profileOverrides);
  const profilesWithoutOverride = configuredProfiles.filter((p) => !(p.name in settings.profileOverrides));

  return (
    <section className="card-padded max-w-md">
      <h3 className="text-sm font-semibold text-primary mb-1">Chat</h3>
      <p className="text-xs text-muted mb-3">
        Which backend answers the interactive Chat page. Separate from "Global manager" above --
        that one drives autonomous wake notifications, not chat.
      </p>

      <label className="block text-xs font-medium text-secondary mb-1">Default backend</label>
      <select
        value={settings.defaultBackend}
        onChange={(e) => save({ defaultBackend: e.target.value })}
        disabled={loading}
        className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary mb-4"
      >
        {backendOptions.map((b) => (
          <option key={b.id} value={b.id} disabled={!b.implemented}>
            {b.displayName}
            {!b.implemented ? ' (coming soon)' : ''}
          </option>
        ))}
      </select>

      <p className="text-xs font-medium text-secondary mb-1">Per-profile overrides</p>
      {overrideEntries.length === 0 ? (
        <p className="text-xs text-muted mb-2">None -- every profile uses the default backend above.</p>
      ) : (
        <ul className="space-y-1.5 mb-2">
          {overrideEntries.map(([profile, backend]) => (
            <li key={profile} className="flex items-center justify-between text-xs bg-raised rounded-md px-2 py-1.5">
              <span>
                <span className="font-mono text-secondary">{profile}</span>{' '}
                <span className="text-muted">→</span> {backendOptions.find((b) => b.id === backend)?.displayName ?? backend}
              </span>
              <button onClick={() => removeOverride(profile)} disabled={loading} className="text-muted hover:text-critical">
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}

      {profilesWithoutOverride.length > 0 && (
        <div className="flex items-center gap-2 mt-2">
          <select
            value={newOverrideProfile}
            onChange={(e) => setNewOverrideProfile(e.target.value)}
            className="flex-1 bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary"
          >
            <option value="">Profile…</option>
            {profilesWithoutOverride.map((p) => (
              <option key={p.name} value={p.name}>
                {p.display_name}
              </option>
            ))}
          </select>
          <select
            value={newOverrideBackend}
            onChange={(e) => setNewOverrideBackend(e.target.value)}
            className="flex-1 bg-raised border border-subtle rounded-md px-2 py-1.5 text-xs text-primary"
          >
            <option value="">Backend…</option>
            {backendOptions.map((b) => (
              <option key={b.id} value={b.id} disabled={!b.implemented}>
                {b.displayName}
                {!b.implemented ? ' (coming soon)' : ''}
              </option>
            ))}
          </select>
          <button
            onClick={addOverride}
            disabled={loading || !newOverrideProfile || !newOverrideBackend}
            className="btn-secondary !text-xs !px-2 !py-1.5"
          >
            Add
          </button>
        </div>
      )}

      {error && <p className="mt-3 text-xs text-critical">Error: {error}</p>}
    </section>
  );
}

function SkillBankSettingsSection() {
  const [skills, setSkills] = useState<SkillSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    gahApi.getSkills()
      .then(({ skills: loaded }) => {
        setSkills(loaded);
        setError(null);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  }, []);

  return (
    <section className="card-padded">
      <h3 className="text-sm font-semibold text-primary mb-1">Central skill bank</h3>
      <p className="text-xs text-muted mb-3">Read-only inventory. Bind skills to projects from Manager Chat.</p>
      {error ? (
        <p className="text-xs text-critical">Failed to load skills: {error}</p>
      ) : skills === null ? (
        <p className="text-xs text-muted">Loading…</p>
      ) : skills.length === 0 ? (
        <EmptyState icon={Info} title="No skills installed" description="Add a versioned skill through the central skill bank API." />
      ) : (
        <div className="divide-y divide-subtle border border-subtle rounded-md">
          {skills.map((skill) => (
            <div key={`${skill.id}@${skill.version}`} className="p-3 text-xs">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium text-primary">{skill.displayName}</span>
                <code className="font-mono text-muted">{skill.id}@{skill.version}</code>
                {skill.bound && <StatusBadge tone="good" label="bound" />}
              </div>
              {skill.description && <p className="text-muted mt-1 break-words">{skill.description}</p>}
              <p className="text-muted mt-1 break-all">
                {skill.backends.length > 0 ? `Backends: ${skill.backends.join(', ')}` : 'All backends'} · {skill.source}
              </p>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

const MEMORY_TIERS = ['L0', 'L1', 'L2'];

interface PolicyDraft {
  budgetChars: string;
  tiers: string[];
}

function policyDraft(policy: MemoryContextPolicy | undefined): PolicyDraft {
  return {
    budgetChars: policy?.budgetChars ? String(policy.budgetChars) : '',
    tiers: policy?.tiers ?? []
  };
}

function policyFromDraft(draft: PolicyDraft): MemoryContextPolicy {
  return {
    ...(draft.budgetChars ? { budgetChars: Number(draft.budgetChars) } : {}),
    ...(draft.tiers.length > 0 ? { tiers: draft.tiers } : {})
  };
}

function TierPicker({ value, onChange }: { value: string[]; onChange: (value: string[]) => void }) {
  return (
    <div className="flex flex-wrap gap-x-3 gap-y-2">
      {MEMORY_TIERS.map((tier) => (
        <label key={tier} className="inline-flex min-h-9 items-center gap-1.5 text-xs text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={value.includes(tier)}
            onChange={(event) => onChange(event.target.checked ? [...value, tier] : value.filter((item) => item !== tier))}
            className="accent-accent"
          />
          {tier}
        </label>
      ))}
    </div>
  );
}

function GatewaySettingsSection({ configuredProfiles }: { configuredProfiles: ProfileSummary[] }) {
  const [settings, setSettings] = useState<GatewaySettingsSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [urlDraft, setUrlDraft] = useState('');
  const [keyDraft, setKeyDraft] = useState('');
  const [enabledDraft, setEnabledDraft] = useState(true);
  const [disabledProfilesDraft, setDisabledProfilesDraft] = useState<string[]>([]);
  const [globalPolicyDraft, setGlobalPolicyDraft] = useState<PolicyDraft>(policyDraft(undefined));
  const [profilePolicyDrafts, setProfilePolicyDrafts] = useState<Record<string, PolicyDraft>>({});
  const [revealKey, setRevealKey] = useState(false);

  const load = () =>
    gahApi
      .getGatewaySettings()
      .then((data) => {
        setSettings(data);
        setUrlDraft(data.url);
        setKeyDraft('');
        setEnabledDraft(data.enabled);
        setDisabledProfilesDraft(data.disabledProfiles);
        setGlobalPolicyDraft(policyDraft(data.contextPolicy));
        setProfilePolicyDrafts(Object.fromEntries(
          Object.entries(data.contextPolicies).map(([name, policy]) => [name, policyDraft(policy)])
        ));
        setError(null);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));

  useEffect(() => { load(); }, []);

  const save = async () => {
    const drafts = [globalPolicyDraft, ...Object.values(profilePolicyDrafts)];
    if (drafts.some((draft) => draft.budgetChars && (!Number.isInteger(Number(draft.budgetChars)) || Number(draft.budgetChars) < 1))) {
      setError('Memory budgets must be whole numbers greater than zero, or blank for unlimited.');
      return;
    }
    setSaving(true);
    try {
      const updated = await gahApi.updateGatewaySettings({
        url: urlDraft || null,
        enabled: enabledDraft,
        disabledProfiles: disabledProfilesDraft,
        contextPolicy: policyFromDraft(globalPolicyDraft),
        contextPolicies: Object.fromEntries(
          Object.entries(profilePolicyDrafts).map(([name, policy]) => [name, policyFromDraft(policy)])
        ),
        ...(keyDraft ? { apiKey: keyDraft } : {})
      });
      setSettings(updated);
      setKeyDraft('');
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  if (!settings) {
    return (
      <section className="card-padded">
        <h3 className="text-sm font-semibold text-primary mb-1">TDAI memory gateway</h3>
        {error ? <p className="text-xs text-critical">Failed to load: {error}</p> : <p className="text-xs text-muted">Loading…</p>}
      </section>
    );
  }

  return (
    <section className="card-padded space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold text-primary">TDAI memory gateway</h3>
          <p className="text-xs text-muted mt-1">Shared recall and capture configuration for every manager backend.</p>
        </div>
        <label className="flex items-center gap-2 text-xs text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={enabledDraft}
            onChange={(e) => setEnabledDraft(e.target.checked)}
            className="accent-accent"
          />
          Enabled
        </label>
      </div>

      <div className="rounded-md border border-subtle p-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-medium text-secondary">Health</span>
          <StatusBadge
            tone={settings.degraded.degraded ? 'critical' : 'good'}
            label={settings.degraded.degraded ? 'degraded' : 'healthy'}
          />
        </div>
        <p className="text-xs text-muted mt-2">
          Last success: {settings.degraded.lastOkAt ? new Date(settings.degraded.lastOkAt).toLocaleString() : 'not observed'}
        </p>
        {settings.degraded.lastFailedAt && (
          <p className="text-xs text-critical mt-1 break-words">
            Last failure: {new Date(settings.degraded.lastFailedAt).toLocaleString()}
            {settings.degraded.lastError ? ` — ${settings.degraded.lastError}` : ''}
          </p>
        )}
      </div>

      <div className="max-w-2xl">
        <label className="block text-xs font-medium text-secondary mb-1">Gateway URL</label>
        <input
          type="text"
          value={urlDraft}
          onChange={(e) => setUrlDraft(e.target.value)}
          placeholder="http://127.0.0.1:8420"
          className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-xs text-primary font-mono focus:outline-none focus:border-accent"
        />
        <p className="text-xs text-muted mt-1">Leave blank to use <code className="font-mono">TDAI_GATEWAY_URL</code> env var.</p>
      </div>

      <div>
        <label className="block text-xs font-medium text-secondary mb-1">New API Key</label>
        <div className="flex items-center gap-2">
          <input
            type={revealKey ? 'text' : 'password'}
            value={keyDraft}
            onChange={(e) => setKeyDraft(e.target.value)}
            placeholder={settings.apiKeyConfigured ? 'Leave blank to keep current key' : 'Enter an API key'}
            className="flex-1 bg-raised border border-subtle rounded-md px-3 py-1.5 text-xs text-primary font-mono focus:outline-none focus:border-accent"
          />
          <button
            type="button"
            onClick={() => setRevealKey((v) => !v)}
            className="btn-secondary !p-2"
            title={revealKey ? 'Hide API key' : 'Reveal API key'}
            aria-label={revealKey ? 'Hide API key' : 'Reveal API key'}
          >
            {revealKey ? <EyeOff size={14} /> : <Eye size={14} />}
          </button>
        </div>
        <p className="text-xs text-muted mt-1">Leave blank to keep the current configured key source.</p>
      </div>

      <fieldset>
        <legend className="text-xs font-medium text-secondary">Profile participation</legend>
        <p className="text-xs text-muted mt-1 mb-2">Checked profiles recall and capture memory. Changes apply on the next turn.</p>
        {configuredProfiles.length === 0 ? (
          <p className="text-xs text-muted">No configured profiles.</p>
        ) : (
          <div className="grid gap-2 sm:grid-cols-2">
            {configuredProfiles.map((profile) => (
              <label key={profile.name} className="flex min-h-9 min-w-0 items-center gap-2 rounded-md border border-subtle px-3 py-2 text-xs text-secondary cursor-pointer">
                <input
                  type="checkbox"
                  checked={!disabledProfilesDraft.includes(profile.name)}
                  onChange={(event) => setDisabledProfilesDraft((current) => event.target.checked
                    ? current.filter((name) => name !== profile.name)
                    : [...current, profile.name])}
                  className="accent-accent"
                />
                <span className="min-w-0 truncate">{profile.display_name} ({profile.name})</span>
              </label>
            ))}
          </div>
        )}
      </fieldset>

      <fieldset className="space-y-3">
        <legend className="text-xs font-medium text-secondary">Global recall policy</legend>
        <p className="text-xs text-muted">Blank budget and no selected tiers mean unlimited characters and all tiers.</p>
        <label className="block max-w-xs text-xs text-secondary">
          Character budget per turn
          <input
            type="number"
            min="1"
            step="1"
            value={globalPolicyDraft.budgetChars}
            onChange={(event) => setGlobalPolicyDraft((current) => ({ ...current, budgetChars: event.target.value }))}
            placeholder="Unlimited"
            className="mt-1 w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-xs text-primary"
          />
        </label>
        <div>
          <span className="block text-xs text-secondary">Eligible tiers</span>
          <TierPicker value={globalPolicyDraft.tiers} onChange={(tiers) => setGlobalPolicyDraft((current) => ({ ...current, tiers }))} />
        </div>
      </fieldset>

      <fieldset className="space-y-3">
        <legend className="text-xs font-medium text-secondary">Per-profile recall overrides</legend>
        <p className="text-xs text-muted">Blank fields inherit the global policy.</p>
        {configuredProfiles.length === 0 ? (
          <p className="text-xs text-muted">No configured profiles.</p>
        ) : (
          <div className="divide-y divide-subtle rounded-md border border-subtle">
            {configuredProfiles.map((profile) => {
              const draft = profilePolicyDrafts[profile.name] ?? policyDraft(undefined);
              const updateDraft = (patch: Partial<PolicyDraft>) => setProfilePolicyDrafts((current) => ({
                ...current,
                [profile.name]: { ...draft, ...patch }
              }));
              return (
                <div key={profile.name} className="grid gap-3 p-3 sm:grid-cols-[minmax(0,1fr)_12rem_minmax(0,1fr)] sm:items-end">
                  <div className="min-w-0">
                    <p className="text-xs font-medium text-primary truncate">{profile.display_name}</p>
                    <p className="text-xs text-muted truncate">{profile.name}</p>
                  </div>
                  <label className="block text-xs text-secondary">
                    Character budget
                    <input
                      type="number"
                      min="1"
                      step="1"
                      value={draft.budgetChars}
                      onChange={(event) => updateDraft({ budgetChars: event.target.value })}
                      placeholder="Inherit"
                      className="mt-1 w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-xs text-primary"
                    />
                  </label>
                  <div>
                    <span className="block text-xs text-secondary">Tier override</span>
                    <TierPicker value={draft.tiers} onChange={(tiers) => updateDraft({ tiers })} />
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </fieldset>

      <div className="flex items-center gap-3">
        <button
          onClick={save}
          disabled={saving}
          className="btn-primary text-xs px-3 py-1.5 disabled:opacity-50"
        >
          {saving ? 'Saving…' : saved ? 'Saved' : 'Save'}
        </button>
        {error && <p className="text-xs text-critical">{error}</p>}
      </div>
    </section>
  );
}

/** Issue #880/#881 follow-up: generates a ready-to-paste command for a
 * genuinely different machine (macOS or Linux -- bootstrap.sh handles
 * both identically, no OS branching needed; Windows gets a one-line WSL
 * note rather than a separate script) to point at this node's compaction
 * db. Deliberately does NOT also generate a full node-registration
 * command (issue #881's `register-node`) -- that requires the central
 * node to be reachable over HTTPS (registerNode() rejects a non-loopback
 * `authenticated_remote` URL that isn't https/wss), which this host isn't
 * set up for yet. */
export function AddNodeSection() {
  const [settings, setSettings] = useState<GatewaySettingsSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [command, setCommand] = useState<string | null>(null);
  const [revealing, setRevealing] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    gahApi
      .getGatewaySettings()
      .then((data) => {
        setSettings(data);
        setError(null);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  }, []);

  if (!settings) {
    return (
      <section className="card-padded max-w-2xl">
        <h3 className="text-sm font-semibold text-primary mb-1">Add a Node</h3>
        {error ? <p className="text-xs text-critical">Failed to load: {error}</p> : <p className="text-xs text-muted">Loading…</p>}
      </section>
    );
  }

  const gatewayPort = (() => {
    try {
      return new URL(settings.url).port || '8420';
    } catch {
      return '8420';
    }
  })();

  const revealCommand = async () => {
    setRevealing(true);
    setError(null);
    try {
      const revealed = await gahApi.revealGatewayBootstrapCommand();
      setCommand(revealed.command);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setRevealing(false);
    }
  };

  const copyCommand = () => {
    if (!command) return;
    navigator.clipboard.writeText(command).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  };

  return (
    <section className="card-padded max-w-2xl">
      <h3 className="text-sm font-semibold text-primary mb-1">Add a Node</h3>
      <p className="text-xs text-muted mb-3">
        Paste on a new machine (macOS or Linux — on Windows, run this inside WSL) to point it at this node's
        compaction db over Tailscale. Installs Rust/Node if missing, clones the repo, and validates the key
        against this gateway before completing — it fails loudly instead of silently succeeding with a bad key.
      </p>
      {!settings.apiKeyConfigured ? (
        <p className="text-xs text-muted">Configure a gateway API key above first.</p>
      ) : !settings.tailscaleIPv4 ? (
        <p className="text-xs text-muted">
          Couldn't detect this host's Tailscale address (is <code className="font-mono">tailscale</code> installed and
          logged in?). Fill in the host yourself:{' '}
          <code className="font-mono">GAH_GATEWAY_URL=http://&lt;this-host&gt;:{gatewayPort}</code>.
        </p>
      ) : command ? (
        <div className="flex items-start gap-2">
          <pre className="flex-1 bg-raised border border-subtle rounded-md px-3 py-2 text-xs text-primary font-mono whitespace-pre-wrap break-all">
            {command}
          </pre>
          <button onClick={copyCommand} className="text-muted hover:text-primary mt-1 shrink-0" title="Copy">
            {copied ? <Check size={14} /> : <Copy size={14} />}
          </button>
        </div>
      ) : (
        <button
          type="button"
          onClick={revealCommand}
          disabled={revealing}
          className="btn-secondary text-xs px-3 py-1.5 disabled:opacity-50"
        >
          {revealing ? 'Revealing…' : 'Reveal setup command'}
        </button>
      )}
      {error && <p className="mt-3 text-xs text-critical">Error: {error}</p>}
    </section>
  );
}

/** Issue #989: in-app "pull, build, restart" path so updating GAH no longer
 * requires SSH. The endpoint restarts this very server on success, so the
 * request that started it can never itself report completion -- this polls
 * `/api/admin/update/status` (backed by a state file that survives the
 * restart, see apps/server/src/adminUpdate.ts) until it reaches a terminal
 * status, tolerating the brief window where the server is down mid-restart,
 * then reloads the page once it's confirmed back up. Renders nothing when
 * the server has the feature disabled (GAH_ENABLE_ADMIN_UPDATE unset). */
export function AdminUpdateSection() {
  const [enabled, setEnabled] = useState(true);
  const [pending, setPending] = useState<AdminUpdatePendingInfo | null>(null);
  const [status, setStatus] = useState<AdminUpdateState | null>(null);
  const [polling, setPolling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    gahApi
      .getAdminUpdatePending()
      .then(setPending)
      .catch((err) => {
        if (err instanceof GahApiError && err.status === 404) {
          setEnabled(false);
          return;
        }
        setError(err instanceof Error ? err.message : String(err));
      });
    gahApi
      .getAdminUpdateStatus()
      .then((data) => {
        setStatus(data);
        if (data.status === 'running') setPolling(true);
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!polling) return;
    let cancelled = false;
    const tick = async () => {
      try {
        const next = await gahApi.getAdminUpdateStatus();
        if (cancelled) return;
        setStatus(next);
        if (next.status === 'running') {
          setTimeout(tick, 2000);
        } else {
          setPolling(false);
          if (next.status === 'success' || next.status === 'inferred_restart') {
            window.location.reload();
          }
        }
      } catch {
        // The restart step briefly takes the server down -- keep polling
        // instead of surfacing a transient fetch failure as an error.
        if (!cancelled) setTimeout(tick, 2000);
      }
    };
    tick();
    return () => {
      cancelled = true;
    };
  }, [polling]);

  const runUpdate = async () => {
    setError(null);
    try {
      const state = await gahApi.startAdminUpdate();
      setStatus(state);
      if (state.status === 'running') setPolling(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  if (!enabled) return null;

  const running = status?.status === 'running';

  return (
    <section className="card-padded max-w-2xl space-y-3">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-semibold text-primary">Update GAH</h3>
        <button onClick={runUpdate} disabled={running} className="btn-primary text-xs px-3 py-1.5 disabled:opacity-50">
          {running ? 'Updating…' : 'Update now'}
        </button>
      </div>
      {pending && (
        <p className="text-xs text-muted font-mono">
          {pending.upToDate
            ? `Up to date at ${pending.current?.short ?? '?'}`
            : `${pending.commitsBehind} commit(s) behind: ${pending.current?.short ?? '?'} → ${pending.latest?.short ?? '?'}`}
        </p>
      )}
      {status && status.status !== 'idle' && (
        <div>
          <p className="text-xs text-secondary">
            Status: {status.status}
            {status.status === 'inferred_restart' && ' — server restarted, reloading…'}
          </p>
          {status.output && (
            <pre className="mt-1 max-h-64 overflow-auto bg-raised border border-subtle rounded-md px-3 py-2 text-xs font-mono whitespace-pre-wrap">
              {status.output}
            </pre>
          )}
        </div>
      )}
      {error && <p className="text-xs text-critical">{error}</p>}
    </section>
  );
}
