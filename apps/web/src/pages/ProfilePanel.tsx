import { useEffect, useRef, useState } from 'react';
import { ExternalLink, Save, Loader2 } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { ExternalAnchor } from '../components/ExternalAnchor';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';
import { ProfileEditor } from '../components/ProfileEditor.js';
import { WorkerScalingSection } from '../components/WorkerScalingSection.js';
import { oldestFetchedAt } from '../lib/format.js';
import { backendInstancesApi, promptPoliciesApi, routingCandidatesApi, GahApiError, type BackendRunnerKind } from '../api/client.js';
import type { WakeAutonomyValue, SettingsConfigProfileSummary, RoutingCandidateSummary } from '@git-agent-harness/contracts';

const PROFILE_REFRESH_MS = 60 * 1000;
const WAKE_AUTONOMY_OPTIONS: { value: WakeAutonomyValue; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'review_only', label: 'Review only' },
  { value: 'full', label: 'Full' },
];

/**
 * The Profile sidebar: which configured repository the dashboard reads from,
 * and everything GAH does for that one profile — dispatch limits, routing
 * candidates, prompt policies, backend accounts, and the profile list itself.
 * App-wide preferences stay in Settings.
 */
export function ProfilePanel() {
  const { providers, providerStatuses, sendMessage, isConnected, profile } = useWebSocket();
  const { profileOverride } = useUiStore();
  const profiles = useGahStore((s) => s.profiles);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  const profileConfig = useGahStore((s) => s.profileConfig);
  const fetchProfileConfig = useGahStore((s) => s.fetchProfileConfig);
  const configuredProfiles = profiles.data ?? [];
  const selectedName = profileOverride ?? profile ?? '';
  const selected = configuredProfiles.find((p) => p.name === selectedName);

  useEffect(() => { fetchProfiles(); }, [fetchProfiles]);
  useEffect(() => {
    if (selectedName) fetchProfileConfig(selectedName);
  }, [selectedName, fetchProfileConfig]);

  const refreshAll = () => {
    fetchProfiles({ force: true });
    if (selectedName) fetchProfileConfig(selectedName, { force: true });
  };
  useAutoRefresh(refreshAll, PROFILE_REFRESH_MS);
  useWsReconnectRefresh(refreshAll);
  const lastUpdated = oldestFetchedAt(profiles.fetchedAt, profileConfig.fetchedAt);

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
      <PageHeader
        title="Profile"
        description="The repository GAH works in, and how it dispatches work there"
        onRefresh={refreshAll}
        refreshing={profiles.loading || profileConfig.loading}
        lastUpdated={lastUpdated}
      />

      <section className="card-padded max-w-4xl" aria-labelledby="current-project-title">
        <h3 id="current-project-title" className="text-sm font-semibold text-primary mb-2">Current project</h3>
        <p className="text-xs text-muted mb-3">Switch projects from the navbar; every page and this sidebar follow it.</p>
        <div className="max-w-md">
          {profiles.loading && !profiles.data ? (
            <p className="text-xs text-muted">Loading configured profiles…</p>
          ) : profiles.error ? (
            <p className="text-xs text-critical">Failed to load profiles: {profiles.error}</p>
          ) : !selected ? (
            <p className="text-xs text-muted">{configuredProfiles.length === 0 ? 'No profiles found in the GAH config.' : 'No project selected.'}</p>
          ) : (
            <p className="text-sm text-primary">{selected.display_name} <span className="text-xs text-muted">({selected.name})</span></p>
          )}
          {selected?.web_url && (
            <ExternalAnchor
              href={selected.web_url}
              className="mt-2 inline-flex items-center gap-1 text-xs text-accent hover:underline"
            >
              <ExternalLink size={12} aria-hidden="true" />
              {selected.repo}
            </ExternalAnchor>
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

      <DispatchSettingsSection
        selectedName={selectedName}
        selected={selected}
        profileLoading={profiles.loading}
        profileError={profiles.error}
      />
      {selected && <WorkerScalingSection selectedName={selectedName} selected={selected} />}
      <ProfileConfigViewerSection
        selectedName={selectedName}
        profileConfig={profileConfig}
        onRefresh={() => fetchProfileConfig(selectedName, { force: true })}
      />
      <section>
        <ProfileEditor />
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
    hold_contract_changes?: boolean;
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
  const [holdContractChanges, setHoldContractChanges] = useState(true);
  // Tracks which profile name the form was last seeded for. We only seed once
  // per profile selection — subsequent data refreshes (e.g. a GET after a
  // PATCH) must not overwrite the user's in-progress edits.
  const seededProfileRef = useRef<string | null>(null);

  const validationTimeoutValue = validationTimeout.trim();
  const parsedValidationTimeout = Number(validationTimeoutValue);
  const validationTimeoutError = validationTimeoutValue !== ''
    && (!Number.isInteger(parsedValidationTimeout) || parsedValidationTimeout < 1)
    ? 'Validation timeout must be a whole number of seconds greater than zero.'
    : null;

  useEffect(() => {
    if (!selected) return;
    if (seededProfileRef.current === selectedName) return;
    seededProfileRef.current = selectedName;
    setParallel(selected.max_parallel_workers != null ? String(selected.max_parallel_workers) : '');
    setValidationTimeout(selected.validation_timeout_seconds != null ? String(selected.validation_timeout_seconds) : '');
    setAutonomy(selected.manager_wake_autonomy ?? 'off');
    setHoldContractChanges(selected.hold_contract_changes ?? true);
  }, [selectedName, selected]);

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
      hold_contract_changes: holdContractChanges,
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

        <div>
          <label className="flex items-center gap-2 text-xs font-medium text-secondary">
            <input
              type="checkbox"
              checked={holdContractChanges}
              onChange={(e) => setHoldContractChanges(e.target.checked)}
            />
            Hold schema and API contract changes for my review
          </label>
          <p className="text-xs text-muted mt-1">
            When on, an approved PR that changes the ledger, telemetry, migrations or
            <code> packages/contracts</code> without compatibility evidence waits for you.
            Other approved PRs never wait on this check.
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

interface ProfileConfigViewerSectionProps {
  selectedName: string;
  profileConfig: {
    data: SettingsConfigProfileSummary | null;
    loading: boolean;
    error: string | null;
  };
  /** Issue #149: refetch after a routing-candidate mutation. */
  onRefresh: () => void;
}

export function ProfileConfigViewerSection({ selectedName, profileConfig, onRefresh }: ProfileConfigViewerSectionProps) {
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
        <EditableCandidateList title="PM candidates" listKey="pm" profile={selectedName} candidates={effective.pm_candidates} onMutate={onRefresh} />
        <EditableCandidateList title="Improve candidates" listKey="improve" profile={selectedName} candidates={effective.improve_candidates} onMutate={onRefresh} />
        <EditableCandidateList title="Review candidates" listKey="review" profile={selectedName} candidates={effective.review_candidates} onMutate={onRefresh} />
      </div>

      <div className="mt-3">
        <BackendInstancesCard profileName={selectedName} effective={effective} />
      </div>

      <div className="mt-3">
        <PromptPoliciesCard profileName={selectedName} effective={effective} onRefresh={onRefresh} />
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

function PromptPoliciesCard({ profileName, effective, onRefresh }: {
  profileName: string;
  effective: SettingsConfigProfileSummary;
  onRefresh: () => void;
}) {
  const [slot, setSlot] = useState<'worker_guidance' | 'reviewer_guidance'>('worker_guidance');
  const [taskClass, setTaskClass] = useState('');
  const [reviewerTier, setReviewerTier] = useState('');
  const [content, setContent] = useState('');
  const [pending, setPending] = useState(false);
  const [preview, setPreview] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const policy = effective.prompt_policies;

  useEffect(() => {
    setContent('');
    setPreview(null);
    setError(null);
  }, [profileName, policy.revision]);

  const target = {
    slot,
    ...(taskClass.trim() ? { task_class: taskClass.trim() } : {}),
    ...(slot === 'reviewer_guidance' && reviewerTier ? { reviewer_tier: reviewerTier } : {}),
  };

  const run = async (operation: () => Promise<import('@git-agent-harness/contracts').PromptPolicyMutationResult>) => {
    setPending(true);
    setError(null);
    try {
      const result = await operation();
      setPreview(result.preview_diff);
      if (!result.dry_run) onRefresh();
    } catch (failure) {
      setError(failure instanceof GahApiError ? failure.message : 'Prompt policy mutation failed. Refresh and retry.');
    } finally {
      setPending(false);
    }
  };

  const overrides = policy.policies.filter((entry) => entry.source === 'profile_override');

  return (
    <div className="card-padded border border-subtle">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h4 className="text-xs font-semibold text-primary">Prompt policies</h4>
          <p className="text-xs text-muted mt-1">
            Revision {policy.revision}. Guidance is bounded and untrusted; safety, approval, merge, evidence, and skill bindings stay protected.
          </p>
        </div>
        {policy.rollback_revisions.length > 0 && (
          <button
            type="button"
            disabled={pending}
            className="btn-secondary text-xs"
            onClick={() => {
              const revision = policy.rollback_revisions.at(-1);
              if (revision != null) void run(() => promptPoliciesApi.rollback(profileName, {
                to_revision: revision,
                expected_revision: policy.revision,
              }));
            }}
          >
            Roll back to r{policy.rollback_revisions.at(-1)}
          </button>
        )}
      </div>

      <ul className="mt-3 space-y-1 text-xs text-secondary">
        {policy.policies.map((entry) => (
          <li className="break-words" key={[entry.slot, entry.task_class ?? 'any', entry.reviewer_tier ?? 'any', entry.source].join('-')}>
            <span className="font-mono">{entry.slot}</span> · task {entry.task_class ?? 'any'} · tier {entry.reviewer_tier ?? 'any'} · {entry.source.replace('_', ' ')} · {entry.version} · {entry.byte_size} bytes · <span className="font-mono break-all">{entry.sha256.slice(0, 19)}…</span>
            {entry.source === 'profile_override' && (
              <button
                type="button"
                disabled={pending}
                className="ml-2 text-critical hover:underline disabled:opacity-50"
                onClick={() => void run(() => promptPoliciesApi.reset(profileName, {
                  slot: entry.slot,
                  ...(entry.task_class ? { task_class: entry.task_class } : {}),
                  ...(entry.reviewer_tier ? { reviewer_tier: entry.reviewer_tier } : {}),
                  expected_revision: policy.revision,
                }))}
              >
                Restore default
              </button>
            )}
          </li>
        ))}
      </ul>

      <div className="mt-3 grid grid-cols-1 md:grid-cols-3 gap-2">
        <label className="text-xs text-secondary">
          Slot
          <select className="mt-1 w-full bg-raised border border-subtle rounded px-2 py-1 text-primary" value={slot} onChange={(event) => { setSlot(event.target.value as typeof slot); setPreview(null); }}>
            <option value="worker_guidance">Worker guidance</option>
            <option value="reviewer_guidance">Reviewer guidance</option>
          </select>
        </label>
        <label className="text-xs text-secondary">
          Task class (optional)
          <input className="mt-1 w-full bg-raised border border-subtle rounded px-2 py-1 text-primary" value={taskClass} maxLength={64} onChange={(event) => { setTaskClass(event.target.value); setPreview(null); }} />
        </label>
        <label className="text-xs text-secondary">
          Reviewer tier
          <select className="mt-1 w-full bg-raised border border-subtle rounded px-2 py-1 text-primary disabled:opacity-50" value={reviewerTier} disabled={slot === 'worker_guidance'} onChange={(event) => { setReviewerTier(event.target.value); setPreview(null); }}>
            <option value="">Any tier</option>
            <option value="strong">Strong</option>
            <option value="escalatory">Escalatory</option>
            <option value="standard">Standard</option>
            <option value="weak">Weak</option>
          </select>
        </label>
      </div>
      <label className="block mt-2 text-xs text-secondary">
        Replacement guidance
        <textarea
          className="mt-1 w-full min-h-24 bg-raised border border-subtle rounded px-2 py-1 text-primary font-mono"
          value={content}
          maxLength={4096}
          onChange={(event) => {
            setContent(event.target.value);
            setPreview(null);
          }}
          placeholder="Enter bounded guidance. Protected instructions cannot be replaced."
        />
      </label>
      <div className="mt-2 flex gap-2">
        <button type="button" className="btn-secondary text-xs" disabled={pending || !content.trim()} onClick={() => void run(() => promptPoliciesApi.set(profileName, {
          ...target,
          content,
          expected_revision: policy.revision,
          dry_run: true,
        }))}>Preview</button>
        <button type="button" className="btn-primary text-xs" disabled={pending || !content.trim() || preview == null} onClick={() => void run(() => promptPoliciesApi.set(profileName, {
          ...target,
          content,
          expected_revision: policy.revision,
        }))}>{pending ? 'Saving…' : 'Apply'}</button>
      </div>
      {error && <p role="alert" className="mt-2 text-xs text-critical">{error}</p>}
      {preview && <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap text-xs text-muted bg-raised border border-subtle rounded p-2">{preview}</pre>}
      {overrides.length === 0 && <p className="mt-2 text-xs text-muted">No profile overrides. Embedded defaults are active.</p>}
    </div>
  );
}

/** Issue #822: per-instance enable/disable. Disabled instances stay declared
 * (status/attribution keep working) but routing skips them with a typed
 * reason. Toggles shell out to the fixed CLI command through the server's
 * owner-gated mutation API; the merged-entry write and validation live in
 * Rust. */
export function BackendInstancesCard({ profileName, effective }: { profileName: string; effective: Pick<SettingsConfigProfileSummary, 'backend_instances'> }) {
  const declared = effective.backend_instances;
  const [instances, setInstances] = useState(declared ?? []);
  const [pendingInstance, setPendingInstance] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadedFor, setLoadedFor] = useState(profileName);
  const [adding, setAdding] = useState(false);
  const [newInstance, setNewInstance] = useState('');
  const [newLabel, setNewLabel] = useState('');
  const [newRunner, setNewRunner] = useState<BackendRunnerKind>('codex');
  const [editing, setEditing] = useState<string | null>(null);
  const [editLabel, setEditLabel] = useState('');
  const [loginCommand, setLoginCommand] = useState<string | null>(null);

  useEffect(() => {
    if (!declared) return;
    let cancelled = false;
    setInstances(declared);
    setLoadedFor(profileName);
    setError(null);
    setAdding(false);
    setEditing(null);
    setLoginCommand(null);
    backendInstancesApi.list(profileName)
      .then((response) => { if (!cancelled) setInstances(response.backend_instances); })
      .catch(() => { if (!cancelled) setError('Auth status is unavailable. Use Test to retry.'); });
    return () => { cancelled = true; };
  }, [declared, profileName]);

  const toggle = async (instance: string, enabled: boolean) => {
    setPendingInstance(instance);
    setError(null);
    try {
      const response = await backendInstancesApi.setEnabled(profileName, instance, enabled);
      setInstances(response.backend_instances);
    } catch (failure) {
      setError(failure instanceof GahApiError ? failure.message : 'Toggle failed. Refresh and retry.');
    } finally {
      setPendingInstance(null);
    }
  };

  const add = async () => {
    setPendingInstance('new');
    setError(null);
    try {
      const response = await backendInstancesApi.add(profileName, newInstance.trim(), newRunner, newLabel.trim());
      setInstances(response.backend_instances);
      setNewInstance('');
      setNewLabel('');
      setAdding(false);
    } catch (failure) {
      setError(failure instanceof GahApiError ? failure.message : 'Account was not added. Refresh and retry.');
    } finally {
      setPendingInstance(null);
    }
  };

  const saveLabel = async (instance: string) => {
    setPendingInstance(instance);
    setError(null);
    try {
      const response = await backendInstancesApi.setLabel(profileName, instance, editLabel.trim());
      setInstances(response.backend_instances);
      setEditing(null);
    } catch (failure) {
      setError(failure instanceof GahApiError ? failure.message : 'Account label was not changed. Refresh and retry.');
    } finally {
      setPendingInstance(null);
    }
  };

  const test = async (instance: string) => {
    setPendingInstance(instance);
    setError(null);
    try {
      const response = await backendInstancesApi.list(profileName);
      setInstances(response.backend_instances);
    } catch (failure) {
      setError(failure instanceof GahApiError ? failure.message : 'Account test failed. Check Doctor for details.');
    } finally {
      setPendingInstance(null);
    }
  };

  const reauthenticate = async (instance: string) => {
    const command = `gah config authenticate-backend-instance --profile ${profileName} --instance ${instance}`;
    setLoginCommand(command);
    await navigator.clipboard?.writeText(command).catch(() => undefined);
  };

  return (
    <div className="card-padded border border-subtle">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h4 className="text-xs font-semibold text-primary">Backend accounts</h4>
        <button type="button" className="btn-secondary text-xs" onClick={() => setAdding((open) => !open)} disabled={pendingInstance !== null}>
          {adding ? 'Cancel' : 'Add account'}
        </button>
      </div>
      <p className="mb-3 max-w-2xl text-xs text-muted">Manage named runner instances on the connected node. Save and select API keys in the owning computer’s desktop Settings. Codex, Claude and Antigravity use their existing OAuth controls.</p>
      {error && <p className="text-xs text-critical mb-2">{error}</p>}
      {loginCommand && (
        <div className="mb-3 rounded-md border border-subtle bg-raised p-2 text-xs text-secondary">
          <p>Run this command in a terminal on this node. It starts the provider's browser or device login.</p>
          <code className="mt-1 block break-all text-primary">{loginCommand}</code>
        </div>
      )}
      {adding && (
        <div className="mb-3 grid gap-2 rounded-md border border-subtle bg-raised p-3 sm:grid-cols-2">
          <label className="space-y-1 text-xs text-secondary">Runner
            <select value={newRunner} onChange={(event) => setNewRunner(event.target.value as BackendRunnerKind)} className="w-full rounded-md border border-subtle bg-card px-2 py-1.5 text-primary">
              <option value="codex">Codex</option><option value="claude">Claude</option><option value="opencode">OpenCode</option><option value="vibe">Vibe</option><option value="openhands">OpenHands</option><option value="hermes">Hermes</option><option value="agy">Antigravity</option>
            </select>
          </label>
          <label className="space-y-1 text-xs text-secondary">Account label
            <input value={newLabel} onChange={(event) => setNewLabel(event.target.value)} placeholder="Personal" className="w-full rounded-md border border-subtle bg-card px-2 py-1.5 text-primary" />
          </label>
          <label className="space-y-1 text-xs text-secondary sm:col-span-2">Instance ID
            <input value={newInstance} onChange={(event) => setNewInstance(event.target.value)} placeholder="codex-personal" className="w-full rounded-md border border-subtle bg-card px-2 py-1.5 font-mono text-primary" />
          </label>
          <button type="button" className="btn-primary text-xs sm:col-span-2 sm:justify-self-start" onClick={add} disabled={!newInstance.trim() || !newLabel.trim() || pendingInstance !== null}>Add isolated account</button>
        </div>
      )}
      {instances.length === 0 && <p className="text-xs text-muted">No named accounts yet. Add one to keep provider logins separate.</p>}
      <ul className="space-y-2">
        {instances.map((instance) => (
          <li key={instance.backend_instance} className="flex flex-col gap-2 border-t border-subtle pt-2 first:border-t-0 first:pt-0 sm:flex-row sm:items-center sm:justify-between">
            <div className="min-w-0">
              <p className="text-xs font-medium text-primary">
                {instance.account_label ?? instance.backend_instance}
                <span className="text-muted"> · {instance.logical_backend} · </span>
                <span className="font-mono text-muted">{instance.backend_instance}</span>
                {!instance.enabled && <span className="ml-2 text-critical">disabled</span>}
              </p>
              <p className={`text-xs ${instance.auth_ready ? 'text-secondary' : 'text-warning'}`}>
                {instance.auth_ready === true ? 'Authenticated' : instance.auth_ready === false ? 'Login required' : 'Auth status unavailable'}
                {instance.isolated_state_configured ? ' · isolated state' : ' · shared state'}
              </p>
              {instance.executable_resolved === false && (
                <p className="text-xs text-critical">{instance.resolution_error ?? 'Executable is not resolved.'}</p>
              )}
              {instance.executable_resolved !== false && !instance.executable_configured && (
                <p className="text-xs text-critical">No executable resolution policy configured.</p>
              )}
              {instance.config_source && (
                <p className="text-xs text-muted">Source: {instance.config_source.replace('_', ' ')}</p>
              )}
              {instance.credential_id && <p className="text-xs text-muted break-all">Credential on this node: {instance.credential_id}{instance.credential_provider ? ` · ${instance.credential_provider}` : ''}</p>}
              {editing === instance.backend_instance && (
                <div className="mt-2 flex gap-2">
                  <input aria-label={`Account label for ${instance.backend_instance}`} value={editLabel} onChange={(event) => setEditLabel(event.target.value)} className="min-w-0 rounded-md border border-subtle bg-raised px-2 py-1 text-xs text-primary" />
                  <button type="button" className="btn-primary text-xs" onClick={() => saveLabel(instance.backend_instance)} disabled={!editLabel.trim() || pendingInstance !== null}>Save</button>
                </div>
              )}
            </div>
            <div className="flex flex-wrap gap-1.5">
              <button type="button" className="btn-secondary text-xs" disabled={pendingInstance !== null} onClick={() => { setEditing(instance.backend_instance); setEditLabel(instance.account_label ?? instance.backend_instance); }}>Rename</button>
              <button type="button" className="btn-secondary text-xs" disabled={pendingInstance !== null} onClick={() => test(instance.backend_instance)}>{pendingInstance === instance.backend_instance ? 'Testing…' : 'Test'}</button>
              <button type="button" className="btn-secondary text-xs" disabled={pendingInstance !== null} onClick={() => reauthenticate(instance.backend_instance)}>Re-authenticate</button>
              <button type="button" disabled={pendingInstance !== null || loadedFor !== profileName} onClick={() => toggle(instance.backend_instance, !instance.enabled)} className={instance.enabled ? 'btn-secondary text-xs' : 'btn-primary text-xs'}>{instance.enabled ? 'Disable' : 'Enable'}</button>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}

const ROUTING_LISTS = ['pm', 'improve', 'review', 'escalatory'] as const;
type RoutingListKey = (typeof ROUTING_LISTS)[number];

/** Issue #149: ordered routing-candidate editing. Mutations shell out to the
 * fixed `gah config routing-candidate` commands through the owner-gated
 * mutation API; the CLI resolves the effective list, validates, and writes
 * the profile-level list wholesale. On success the caller refetches. */
function EditableCandidateList({ title, listKey, profile, candidates, onMutate }: {
  title: string;
  listKey: RoutingListKey;
  profile: string;
  candidates: RoutingCandidateSummary[];
  onMutate: () => void;
}) {
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [newBackend, setNewBackend] = useState('');
  const [newModel, setNewModel] = useState('');

  const runMutation = async (key: string, mutation: () => Promise<unknown>) => {
    setPending(key);
    setError(null);
    try {
      await mutation();
      onMutate();
    } catch (failure) {
      setError(failure instanceof GahApiError ? failure.message : 'Mutation failed. Refresh and retry.');
    } finally {
      setPending(null);
    }
  };

  const add = () => {
    const backend = newBackend.trim();
    if (!backend) return;
    setAdding(false);
    void runMutation('add', () =>
      routingCandidatesApi.add(profile, {
        list: listKey,
        backend,
        ...(newModel.trim() !== '' ? { model: newModel.trim() } : {}),
      }));
    setNewBackend('');
    setNewModel('');
  };

  return (
    <div className="card-padded border border-subtle">
      <h4 className="text-xs font-semibold text-primary mb-2">{title}</h4>
      {error && <p className="text-xs text-critical mb-2">{error}</p>}
      {candidates.length === 0 ? (
        <p className="text-xs text-muted mb-2">No candidates configured.</p>
      ) : (
        <ul className="space-y-1.5">
          {candidates.map((candidate, index) => (
            <li key={`${candidate.backend}-${candidate.model ?? 'none'}-${index}`} className="flex items-center justify-between gap-2 text-xs">
              <span className="text-secondary min-w-0 truncate">
                {formatCandidateLabel(candidate)}
                <span className="text-muted">
                  {' '}· priority {candidate.priority}
                  {candidate.requires_approval ? ' · requires approval' : ''}
                </span>
              </span>
              <span className="inline-flex items-center gap-1 shrink-0">
                <button
                  type="button"
                  title="Move up"
                  disabled={pending !== null || index === 0}
                  onClick={() => runMutation(`up-${index}`, () => routingCandidatesApi.move(profile, index, index - 1))}
                  className="px-1.5 py-0.5 border border-subtle rounded text-secondary hover:text-primary disabled:opacity-40"
                >
                  ↑
                </button>
                <button
                  type="button"
                  title="Move down"
                  disabled={pending !== null || index === candidates.length - 1}
                  onClick={() => runMutation(`down-${index}`, () => routingCandidatesApi.move(profile, index, index + 1))}
                  className="px-1.5 py-0.5 border border-subtle rounded text-secondary hover:text-primary disabled:opacity-40"
                >
                  ↓
                </button>
                <button
                  type="button"
                  title="Remove"
                  disabled={pending !== null}
                  onClick={() => runMutation(`rm-${index}`, () => routingCandidatesApi.remove(profile, index))}
                  className="px-1.5 py-0.5 border border-critical/40 rounded text-critical hover:bg-critical/10 disabled:opacity-40"
                >
                  ✕
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
      {adding ? (
        <div className="mt-2 space-y-1.5">
          <input
            type="text"
            autoFocus
            value={newBackend}
            onChange={(e) => setNewBackend(e.target.value)}
            placeholder="backend, e.g. codex"
            className="w-full bg-raised border border-subtle rounded px-2 py-1 text-xs text-primary"
          />
          <input
            type="text"
            value={newModel}
            onChange={(e) => setNewModel(e.target.value)}
            placeholder="model (optional)"
            className="w-full bg-raised border border-subtle rounded px-2 py-1 text-xs text-primary"
          />
          <div className="flex gap-2">
            <button
              type="button"
              onClick={add}
              disabled={newBackend.trim() === '' || pending !== null}
              className="px-2 py-1 bg-accent text-white rounded text-xs font-medium disabled:opacity-50"
            >
              Add
            </button>
            <button
              type="button"
              onClick={() => setAdding(false)}
              className="px-2 py-1 border border-subtle rounded text-xs text-secondary"
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => setAdding(true)}
          disabled={pending !== null}
          className="mt-2 px-2 py-1 border border-subtle rounded text-xs text-secondary hover:text-primary disabled:opacity-40"
        >
          + Add candidate
        </button>
      )}
    </div>
  );
}

function formatCandidateLabel(candidate: RoutingCandidateSummary): string {
  return `${candidate.backend}/${candidate.model ?? 'unknown'}`;
}

function formatList(values: string[]): string {
  return values.length > 0 ? values.join(', ') : 'any';
}

