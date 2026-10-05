import { useEffect, useRef, useState } from 'react';
import { Sun, Moon, Info, Save, Loader2, Eye, EyeOff, Copy, Check, ChevronRight, Search } from 'lucide-react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { useUiStore } from '../store/uiStore.js';
import { useGahStore } from '../store/gahStore.js';
import { useAutoRefresh } from '../hooks/useAutoRefresh.js';
import { useWsReconnectRefresh } from '../hooks/useWsReconnectRefresh.js';
import { CoordinatorConnection } from '../components/CoordinatorConnection.js';
import { FRONTEND_BUILD } from '../components/Navbar.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { EmptyState } from '../components/ui/EmptyState.js';
import { SkillBankSettingsSection } from '../components/SkillBankSettingsSection.js';
import { StatusBadge } from '../components/ui/StatusBadge.js';
import { oldestFetchedAt, formatAge, isStale } from '../lib/format.js';
import { gahApi, backendInstancesApi, GahApiError } from '../api/client.js';
import type { ConfigSetData, NotificationSettingsSummary } from '@git-agent-harness/contracts';
import type { ManagerChatSettingsSummary, ProfileSummary, GatewaySettingsSummary, MemoryContextPolicy, AdminUpdatePendingInfo, AdminUpdateState, BackendInstanceSummary, HelperRoutePreference, ManagerModelInfo } from '@git-agent-harness/contracts';

const SETTINGS_REFRESH_MS = 60 * 1000;
const SETTINGS_SECTIONS_KEY = 'gah.settings.openSections';
type SettingsSectionId = 'general' | 'skills' | 'memory';
const SETTINGS_SECTION_IDS: SettingsSectionId[] = ['general', 'skills', 'memory'];
const SETTINGS_SECTION_TITLES: Record<SettingsSectionId, string> = { general: 'General', skills: 'Skill bank', memory: 'TDAI / memory' };

/** What the search bar finds: each card's heading, the section that holds it
 * (none for the cards above the sections), and words people look for it by.
 * Per-profile dispatch and routing live in the Profile sidebar, not here. */
const SETTINGS_INDEX: { heading: string; section: SettingsSectionId | null; keywords: string }[] = [
  { heading: 'Connection & pairing', section: null, keywords: 'access token device pair qr central server' },
  { heading: 'Appearance', section: 'general', keywords: 'theme dark light notifications popup bell' },
  { heading: 'Global manager', section: 'general', keywords: 'manager wake autonomy' },
  { heading: 'Notification channel', section: 'general', keywords: 'notifications alerts telegram' },
  { heading: 'Chat', section: 'general', keywords: 'manager chat backend model helper routing' },
  { heading: 'Update GAH', section: 'general', keywords: 'version upgrade release' },
  { heading: 'Agent backends', section: 'general', keywords: 'providers eligibility availability quota' },
  { heading: 'Central skill bank', section: 'skills', keywords: 'skills versions' },
  { heading: 'TDAI memory gateway', section: 'memory', keywords: 'memory recall context policy credentials' },
  { heading: 'Memory gateway', section: 'memory', keywords: 'memory setup install' }
];

export function SettingsPage() {
  const { serverVersion, profile } = useWebSocket();
  const { theme, setTheme, notificationPopups, setNotificationPopups, profileOverride } = useUiStore();
  const profiles = useGahStore((s) => s.profiles);
  const fetchProfiles = useGahStore((s) => s.fetchProfiles);
  const config = useGahStore((s) => s.config);
  const fetchConfig = useGahStore((s) => s.fetchConfig);
  const setConfig = useGahStore((s) => s.setConfig);
  const clearConfigErrors = useGahStore((s) => s.clearConfigErrors);
  const quota = useGahStore((s) => s.quota);
  const fetchQuota = useGahStore((s) => s.fetchQuota);
  const configuredProfiles = profiles.data ?? [];
  const selectedName = profileOverride ?? profile ?? '';
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
    if (selectedName) fetchQuota({ profile: selectedName, since: '7d' });
  }, [selectedName, fetchQuota]);

  const refreshAll = () => {
    fetchProfiles({ force: true });
    fetchConfig({ force: true });
    if (selectedName) fetchQuota({ profile: selectedName, since: '7d' }, { force: true });
  };
  useAutoRefresh(refreshAll, SETTINGS_REFRESH_MS);
  useWsReconnectRefresh(refreshAll);
  const lastUpdated = oldestFetchedAt(profiles.fetchedAt, config.fetchedAt);

  const backendSnapshot = quota.data?.profile.profile === selectedName ? quota.data : null;
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

  const root = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState('');
  const [wantedHeading, setWantedHeading] = useState<string | null>(null);
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const matches = terms.length === 0 ? [] : SETTINGS_INDEX.filter((entry) => {
    const text = `${entry.heading} ${entry.keywords}`.toLowerCase();
    return terms.every((term) => text.includes(term));
  });
  // A section renders its cards after it opens, some only once their data loads.
  useEffect(() => {
    if (!wantedHeading) return;
    let tries = 0;
    const timer = window.setInterval(() => {
      const heading = Array.from(root.current?.querySelectorAll('h2, h3') ?? []).find((element) => element.textContent?.trim().startsWith(wantedHeading));
      if (heading) heading.scrollIntoView({ block: 'start' });
      if (heading || ++tries >= 20) { window.clearInterval(timer); setWantedHeading(null); }
    }, 100);
    return () => window.clearInterval(timer);
  }, [wantedHeading]);
  const openResult = (entry: typeof SETTINGS_INDEX[number]) => {
    if (entry.section) setSectionOpen(entry.section, true);
    setQuery('');
    setWantedHeading(entry.heading);
  };

  return (
    <div className="space-y-6" ref={root}>
      <PageHeader
        title="Settings"
        description="Connection, appearance, chat, skills, and memory"
        onRefresh={refreshAll}
        refreshing={profiles.loading || config.loading}
        lastUpdated={lastUpdated}
      />

      <div className="max-w-4xl">
        <label className="relative block">
          <Search size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted" aria-hidden="true" />
          <input type="search" value={query} onChange={(event) => setQuery(event.target.value)}
            aria-label="Search settings" placeholder="Search settings"
            className="w-full rounded-md border border-subtle bg-raised py-2 pl-9 pr-3 text-sm text-primary placeholder:text-muted focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent" />
        </label>
        {terms.length > 0 && (
          <ul className="card mt-2 divide-y divide-subtle" aria-label="Matching settings">
            {matches.length === 0 && <li className="px-3 py-2 text-sm text-muted">No settings match.</li>}
            {matches.map((entry) => (
              <li key={entry.heading}>
                <button type="button" onClick={() => openResult(entry)} className="flex min-h-11 w-full items-center justify-between gap-3 px-3 py-2 text-left text-sm text-primary hover:bg-white/5">
                  {entry.heading}
                  {entry.section && <span className="text-xs text-muted">{SETTINGS_SECTION_TITLES[entry.section]}</span>}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <section className="card-padded max-w-4xl" aria-labelledby="connection-settings-title">
        <h2 id="connection-settings-title" className="text-base font-semibold text-primary">Connection & pairing</h2>
        <p className="mt-1 mb-3 break-all text-sm text-secondary">{window.location.origin}</p>
        <CoordinatorConnection />
        {window.__GAH_DESKTOP_SETTINGS__ === true && (
          <div className="mt-3 border-t border-subtle pt-2">
            <a href="gah://settings" className="flex min-h-11 items-center justify-between gap-3 text-sm font-medium text-accent hover:underline">
              This computer
              <ChevronRight size={17} aria-hidden="true" />
            </a>
            <p className="text-xs text-muted">Server address, local worker, and app presence</p>
          </div>
        )}
        <p className="mt-3 text-xs text-muted" data-testid="settings-build">App {FRONTEND_BUILD}</p>
      </section>

      <div className="max-w-4xl space-y-3">
        <div className="grid gap-3 sm:grid-cols-2">
          <SettingsSectionButton id="general" title={SETTINGS_SECTION_TITLES.general} description="Appearance, chat, updates, and backends." open={openSections.has('general')} onToggle={setSectionOpen} />
          <SettingsSectionButton id="skills" title={SETTINGS_SECTION_TITLES.skills} description="Versioned skills available to backends." open={openSections.has('skills')} onToggle={setSectionOpen} />
          <SettingsSectionButton id="memory" title={SETTINGS_SECTION_TITLES.memory} description="Gateway health, credentials, and recall policy." open={openSections.has('memory')} onToggle={setSectionOpen} />
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
        <label className="mt-4 flex items-start gap-2 text-sm text-primary">
          <input type="checkbox" checked={notificationPopups} onChange={(event) => setNotificationPopups(event.target.checked)} className="mt-1" />
          <span>
            Pop up new notifications
            <span className="block text-xs text-muted">On: a new notification opens under the bell for 3 seconds. Off: it only adds to the bell's counter. Either way it waits in the Notifications menu until you clear it.</span>
          </span>
        </label>
      </section>

      <GlobalManagerSection
        config={config}
        setConfig={setConfig}
        clearConfigErrors={clearConfigErrors}
      />

      <NotificationChannelSection
        config={config}
        setConfig={setConfig}
        clearConfigErrors={clearConfigErrors}
      />

      <ManagerChatSettingsSection configuredProfiles={configuredProfiles} />
      <AdminUpdateSection />
      <section aria-labelledby="agent-availability-title">
        <h3 id="agent-availability-title" className="text-sm font-semibold text-primary mb-3">Agent backends {serverVersion && <span className="text-muted font-normal">· server v{serverVersion}</span>}</h3>
        <p className="text-sm text-secondary mb-3">Routing eligibility from the latest Quota snapshot.</p>
        {backendSnapshot && <p className="text-xs text-muted mb-3">Snapshot <time dateTime={backendSnapshot.generated_at}>{formatAge(backendSnapshot.generated_at) ?? backendSnapshot.generated_at}</time></p>}
        {quota.error && <p role="alert" className="text-sm text-critical mb-3">Cannot refresh backend availability: {quota.error}. {backendSnapshot ? 'Showing the last snapshot.' : ''} Use Refresh to retry.</p>}
        {!backendSnapshot ? <p role="status" className="text-sm text-muted">{quota.loading ? 'Loading backend availability…' : 'No backend availability snapshot.'}</p>
          : backendSnapshot.candidates.length === 0 ? <EmptyState icon={Info} title="No canonical candidates recorded" description="Add routing candidates to this profile to see eligibility." />
          : <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {backendSnapshot.candidates.map((candidate, index) => <article key={index} className="card-padded min-w-0">
              <div className="flex flex-wrap items-start justify-between gap-2">
                <h4 className="text-sm font-medium text-primary break-words">{[candidate.backend, candidate.quota_pool, candidate.model].filter(Boolean).join(' / ')}</h4>
                <StatusBadge tone={candidate.eligible_now ? 'good' : 'critical'} label={candidate.eligible_now ? 'Eligible' : 'Unavailable'} />
              </div>
              {!candidate.eligible_now && <p className="text-sm text-secondary mt-2">Reason: {candidate.reason ?? 'Unknown'}</p>}
              <p className="text-xs text-muted mt-2">{candidate.observed_at ? <>Observed <time dateTime={candidate.observed_at}>{formatAge(candidate.observed_at) ?? candidate.observed_at}</time></> : 'No observation'}</p>
              {isStale(candidate.observed_at) && <StatusBadge tone="serious" label="Stale" />}
            </article>)}
          </div>}
      </section>
        </SettingsSectionPanel>}

        {openSections.has('skills') && <SettingsSectionPanel id="skills">
          <SkillBankSettingsSection />
        </SettingsSectionPanel>}

        {openSections.has('memory') && <SettingsSectionPanel id="memory">
          <GatewaySettingsSection configuredProfiles={configuredProfiles} />
          <GatewaySetupSection />
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

interface GlobalManagerSectionProps {
  config: {
    data: {
      current_manager: string | null;
      notifications?: NotificationSettingsSummary;
    } | null;
    loading: boolean;
    error: string | null;
  };
  setConfig: (data: ConfigSetData) => Promise<void>;
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

const NOTIFICATION_CHANNELS: { value: 'none' | 'telegram' | 'discord'; label: string; hint: string }[] = [
  { value: 'none', label: 'None', hint: 'No channel delivery; per-profile notify_command still applies.' },
  { value: 'telegram', label: 'Telegram', hint: 'Bot API sendMessage. Requires TELEGRAM_BOT_TOKEN in the server environment.' },
  { value: 'discord', label: 'Discord', hint: 'Incoming webhook. Requires DISCORD_WEBHOOK_URL in the server environment.' }
];

/** Issue #653: where notify-worthy events (terminal failures, paid-route and
 * external-approval requests, review verdicts) are delivered in addition to
 * any per-profile notify_command. Credentials live in the server environment
 * (TELEGRAM_BOT_TOKEN / DISCORD_WEBHOOK_URL) — never in config or this UI. */
function NotificationChannelSection({ config, setConfig, clearConfigErrors }: GlobalManagerSectionProps) {
  const notifications = config.data?.notifications;
  const [channel, setChannel] = useState<'none' | 'telegram' | 'discord'>('none');
  const [chatId, setChatId] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setChannel(notifications?.channel ?? 'none');
    setChatId(notifications?.telegram_chat_id ?? '');
  }, [notifications?.channel, notifications?.telegram_chat_id]);

  const selected = NOTIFICATION_CHANNELS.find((entry) => entry.value === channel);

  const handleSave = async () => {
    setSaving(true);
    try {
      await setConfig({
        notification_channel: channel,
        telegram_chat_id: channel === 'telegram' && chatId.trim() !== '' ? chatId.trim() : null,
        clear: channel === 'telegram' ? [] : ['telegram_chat_id'],
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="card-padded max-w-md">
      <h3 className="text-sm font-semibold text-primary mb-1">Notification channel</h3>
      <p className="text-xs text-muted mb-3">
        Where notify-worthy events are delivered in addition to any per-profile
        notify_command. Messages are the same redacted one-liners the dashboard
        events page records; delivery failures are visible there and never block dispatch.
      </p>

      <label className="block text-xs font-medium text-secondary mb-1">Channel</label>
      <select
        value={channel}
        onChange={(e) => setChannel(e.target.value as 'none' | 'telegram' | 'discord')}
        className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
      >
        {NOTIFICATION_CHANNELS.map((entry) => (
          <option key={entry.value} value={entry.value}>
            {entry.label}
          </option>
        ))}
      </select>
      {selected && <p className="text-xs text-muted mt-1">{selected.hint}</p>}

      {channel === 'telegram' && (
        <>
          <label className="block text-xs font-medium text-secondary mb-1 mt-3">
            Telegram chat ID
          </label>
          <input
            type="text"
            value={chatId}
            onChange={(e) => setChatId(e.target.value)}
            placeholder="e.g. 123456789 (from @userinfobot)"
            className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary"
          />
          <p className="text-xs text-muted mt-1">
            Non-secret. The bot token must be set as TELEGRAM_BOT_TOKEN in the server environment — it is never stored in config or shown here.
          </p>
        </>
      )}

      {channel !== 'none' && notifications?.credential_env && (
        <p className="text-xs text-critical mt-2">
          Requires {notifications.credential_env} in the server environment; without it, delivery failures appear on the Events page.
        </p>
      )}

      {config.error && <p className="mt-3 text-xs text-critical">Error: {config.error}</p>}

      <button
        onClick={handleSave}
        disabled={config.loading || saving}
        className="mt-3 inline-flex items-center gap-1.5 px-3 py-1.5 bg-accent text-white rounded-md text-sm font-medium hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        {saving || config.loading ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <Save size={14} aria-hidden="true" />}
        {saving || config.loading ? 'Saving…' : 'Save notification channel'}
      </button>
    </section>
  );
}

const accountValue = (backend: string, instance: string | null) => `${backend}:${instance ?? ''}`;
const parseAccount = (value: string) => {
  const [backend, instance = ''] = value.split(':');
  return { backend, instance: instance || null };
};

function HelperRoutingCard({ profiles, settings, disabled, onSave }: {
  profiles: ProfileSummary[];
  settings: ManagerChatSettingsSummary;
  disabled: boolean;
  onSave: (routes: HelperRoutePreference[]) => void;
}) {
  const [profile, setProfile] = useState(profiles[0]?.name ?? '');
  const [instances, setInstances] = useState<BackendInstanceSummary[]>([]);
  const [source, setSource] = useState('');
  const [draft, setDraft] = useState<HelperRoutePreference | null>(null);
  const [models, setModels] = useState<ManagerModelInfo[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!profile) return;
    let cancelled = false;
    backendInstancesApi.list(profile)
      .then(result => { if (!cancelled) setInstances(result.backend_instances.filter(instance => instance.enabled)); })
      .catch(() => { if (!cancelled) setInstances([]); });
    const backend = settings.profileOverrides[profile] ?? settings.defaultBackend;
    setSource(accountValue(backend, null));
    return () => { cancelled = true; };
  }, [profile, settings.defaultBackend, settings.profileOverrides]);

  const accounts = [
    ...settings.availableBackends.filter(option => option.implemented).map(option => ({
      value: accountValue(option.id, null), label: `${option.displayName} default`, backend: option.id, instance: null as string | null
    })),
    ...instances.map(instance => ({
      value: accountValue(instance.logical_backend, instance.backend_instance),
      label: `${instance.account_label ?? instance.backend_instance} · ${instance.logical_backend}`,
      backend: instance.logical_backend,
      instance: instance.backend_instance as string | null
    }))
  ];

  useEffect(() => {
    if (!profile || !source) return;
    const selected = parseAccount(source);
    const saved = settings.helperRoutes.find(route => route.profile === profile && route.sourceBackend === selected.backend
      && route.sourceBackendInstance === selected.instance);
    setDraft(saved ?? {
      profile, sourceBackend: selected.backend, sourceBackendInstance: selected.instance,
      enabled: true, backend: selected.backend, backendInstance: selected.instance, model: null
    });
  }, [profile, source, settings.helperRoutes]);

  useEffect(() => {
    if (!draft?.enabled) { setModels([]); return; }
    let cancelled = false;
    setLoadingModels(true);
    setError(null);
    gahApi.getManagerChatModelsForBackend(profile, draft.backend, undefined, draft.backendInstance)
      .then(result => { if (!cancelled) setModels(result.models); })
      .catch(cause => { if (!cancelled) { setModels([]); setError(cause instanceof Error ? cause.message : String(cause)); } })
      .finally(() => { if (!cancelled) setLoadingModels(false); });
    return () => { cancelled = true; };
  }, [profile, draft?.enabled, draft?.backend, draft?.backendInstance]);

  if (profiles.length === 0 || !draft) return null;
  const effectiveModel = draft.model ?? (draft.backend === 'codex'
    ? models.find(model => /luna/i.test(`${model.id} ${model.name}`))?.id ?? null
    : null);
  const effective = !draft.enabled ? 'disabled; deterministic fallback'
    : loadingModels ? 'loading model catalog…'
      : error ? 'model catalog unavailable; deterministic fallback'
        : effectiveModel ? `${draft.backendInstance ?? draft.backend} · ${effectiveModel}`
          : draft.backend === 'codex' ? 'no advertised Luna; deterministic fallback'
            : 'no helper model selected; deterministic fallback';
  const target = accountValue(draft.backend, draft.backendInstance);

  return (
    <div className="mt-5 border-t border-subtle pt-4">
      <h4 className="text-xs font-semibold text-primary">Low-cost helper model</h4>
      <p className="mt-1 text-xs text-muted">Chat titles and requested Git prose use this account without changing the coding model.</p>
      <div className="mt-3 grid gap-3 sm:grid-cols-2">
        <label className="space-y-1 text-xs text-secondary">Profile
          <select value={profile} onChange={event => setProfile(event.target.value)} className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-primary">
            {profiles.map(option => <option key={option.name} value={option.name}>{option.display_name || option.name}</option>)}
          </select>
        </label>
        <label className="space-y-1 text-xs text-secondary">Active account route
          <select value={source} onChange={event => setSource(event.target.value)} className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-primary">
            {accounts.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
          </select>
        </label>
        <label className="flex items-center gap-2 text-xs text-secondary sm:col-span-2">
          <input type="checkbox" checked={draft.enabled} onChange={event => setDraft({ ...draft, enabled: event.target.checked })} />
          Use model-generated helper suggestions
        </label>
        <label className="space-y-1 text-xs text-secondary">Helper account
          <select value={target} disabled={!draft.enabled} onChange={event => {
            const selected = parseAccount(event.target.value);
            setDraft({ ...draft, backend: selected.backend, backendInstance: selected.instance, model: null });
          }} className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-primary disabled:opacity-50">
            {accounts.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
          </select>
        </label>
        <label className="space-y-1 text-xs text-secondary">Helper model
          <select value={draft.model ?? ''} disabled={!draft.enabled || loadingModels} onChange={event => setDraft({ ...draft, model: event.target.value || null })}
            className="w-full rounded-md border border-subtle bg-raised px-2 py-1.5 text-primary disabled:opacity-50">
            <option value="">{draft.backend === 'codex' ? 'Automatic advertised Luna' : 'Deterministic fallback'}</option>
            {models.map(model => <option key={model.id} value={model.id}>{model.name}</option>)}
          </select>
        </label>
      </div>
      <p className="mt-2 text-xs text-muted">Effective: {effective}</p>
      {error && <p role="alert" className="mt-2 text-xs text-critical">Model catalog unavailable: {error}</p>}
      <button type="button" className="btn-secondary mt-3 text-xs" disabled={disabled} onClick={() => onSave([
        ...settings.helperRoutes.filter(route => !(route.profile === draft.profile && route.sourceBackend === draft.sourceBackend && route.sourceBackendInstance === draft.sourceBackendInstance)),
        draft
      ])}>Save helper route</button>
    </div>
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
        setSettings({ ...data, helperRoutes: data.helperRoutes ?? [] });
        setError(null);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  };

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useWsReconnectRefresh(load);

  const save = async (update: { defaultBackend?: string; profileOverrides?: Record<string, string>; helperRoutes?: HelperRoutePreference[] }) => {
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

      <HelperRoutingCard profiles={configuredProfiles} settings={settings} disabled={loading} onSave={helperRoutes => void save({ helperRoutes })} />

      {error && <p className="mt-3 text-xs text-critical">Error: {error}</p>}
    </section>
  );
}

const MEMORY_TIERS = ['L0', 'L1', 'L2'];

interface PolicyDraft {
  settleIdleSeconds: string;
  budgetChars: string;
  tiers: string[];
}

function policyDraft(policy: MemoryContextPolicy | undefined): PolicyDraft {
  return {
    settleIdleSeconds: policy?.settleIdleSeconds ? String(policy.settleIdleSeconds) : '',
    budgetChars: policy?.budgetChars ? String(policy.budgetChars) : '',
    tiers: policy?.tiers ?? []
  };
}

function policyFromDraft(draft: PolicyDraft): MemoryContextPolicy {
  return {
    ...(draft.settleIdleSeconds ? { settleIdleSeconds: Number(draft.settleIdleSeconds) } : {}),
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
  // Recover a failed initial read without replacing unsaved settings on reconnect.
  useWsReconnectRefresh(() => { if (!settings) void load(); });

  const save = async () => {
    const drafts = [globalPolicyDraft, ...Object.values(profilePolicyDrafts)];
    if (drafts.some((draft) => draft.budgetChars && (!Number.isInteger(Number(draft.budgetChars)) || Number(draft.budgetChars) < 1))) {
      setError('Memory budgets must be whole numbers greater than zero, or blank for unlimited.');
      return;
    }
    if (drafts.some((draft) => draft.settleIdleSeconds && (!Number.isInteger(Number(draft.settleIdleSeconds)) || Number(draft.settleIdleSeconds) < 1 || Number(draft.settleIdleSeconds) > 86400))) {
      setError('Idle timeouts must be 1–86400 seconds, or blank to use the default.');
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
        <p className="text-xs text-muted mt-1">
          Failed captures: {settings.degraded.captureFailures ?? 0} server, {settings.degraded.hookCaptureFailures ?? 0} local hooks
        </p>
        {settings.degraded.lastFailedAt && (
          <p className="text-xs text-critical mt-1 break-words">
            Last failure: {new Date(settings.degraded.lastFailedAt).toLocaleString()}
            {settings.degraded.lastError ? ` — ${settings.degraded.lastError}` : ''}
          </p>
        )}
      </div>

      <div className="max-w-2xl">
        <label htmlFor="gateway-url" className="block text-xs font-medium text-secondary mb-1">Gateway URL</label>
        <input
          id="gateway-url"
          type="text"
          value={urlDraft}
          onChange={(e) => setUrlDraft(e.target.value)}
          placeholder="http://127.0.0.1:8420"
          className="w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-xs text-primary font-mono focus:outline-none focus:border-accent"
        />
        <p className="text-xs text-muted mt-1">Leave blank to use <code className="font-mono">TDAI_GATEWAY_URL</code> env var.</p>
      </div>

      <div>
        <label htmlFor="gateway-api-key" className="block text-xs font-medium text-secondary mb-1">New API Key</label>
        <div className="flex items-center gap-2">
          <input
            id="gateway-api-key"
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
        <label className="block max-w-xs text-xs text-secondary">
          Save memory after idle (seconds)
          <input type="number" min="1" max="86400" step="1"
            value={globalPolicyDraft.settleIdleSeconds}
            onChange={(event) => setGlobalPolicyDraft((current) => ({ ...current, settleIdleSeconds: event.target.value }))}
            placeholder="900 (15 minutes)"
            className="mt-1 w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-xs text-primary" />
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
                <div key={profile.name} className="grid gap-3 p-3 sm:grid-cols-2 lg:grid-cols-4 sm:items-end">
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
                  <label className="block text-xs text-secondary">
                    Save after idle (seconds)
                    <input type="number" min="1" max="86400" step="1"
                      value={draft.settleIdleSeconds}
                      onChange={(event) => updateDraft({ settleIdleSeconds: event.target.value })}
                      placeholder="Inherit"
                      className="mt-1 w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-xs text-primary" />
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

export function AddNodeSection() {
  const [centralUrl, setCentralUrl] = useState(window.location.origin);
  const [os, setOs] = useState<'windows' | 'linux' | 'macos'>('windows');
  const [role, setRole] = useState<'desktop' | 'worker' | 'both' | 'central' | 'standalone'>('both');
  const [gatewayUrl, setGatewayUrl] = useState('');
  const osName = { windows: 'Windows', linux: 'Linux', macos: 'macOS' }[os];
  const [command, setCommand] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const reveal = async () => {
    setBusy(true); setError(''); setCommand(''); setCopied(false);
    try {
      setCommand((await gahApi.getNodeSetupCommand({ os, centralUrl, role, ...((role === 'central' || role === 'standalone') && gatewayUrl ? { gatewayUrl } : {}) })).command);
    } catch (err) { setError(err instanceof Error ? err.message : String(err)); }
    finally { setBusy(false); }
  };
  return (
    <section className="card-padded max-w-2xl">
      <h3 className="text-sm font-semibold text-primary mb-1">Add a Node</h3>
      <p className="text-xs text-muted mb-3">Choose the new computer’s operating system and role, then copy its install command.</p>
      <label className="block text-xs text-secondary mb-3">Operating system
        <select disabled={busy} className="input w-full mt-1 min-h-11" value={os} onChange={(event) => { setOs(event.target.value as typeof os); setRole(event.target.value === 'windows' ? 'both' : 'worker'); setCommand(''); setError(''); setCopied(false); }}>
          <option value="windows">Windows</option><option value="linux">Linux</option><option value="macos">macOS</option>
        </select>
      </label>
      {role !== 'central' && role !== 'standalone' && <label className="block text-xs text-secondary mb-3">Central LAN or VPN address
        <input disabled={busy} type="url" className="input w-full mt-1 min-h-11" value={centralUrl} onChange={(event) => { setCentralUrl(event.target.value); setCommand(''); }} placeholder="http://192.168.1.10:3773" />
      </label>}
      <label className="block text-xs text-secondary mb-3">Install
        <select disabled={busy} className="input w-full mt-1 min-h-11" value={role} onChange={(event) => { setRole(event.target.value as typeof role); setCommand(''); }}>
          {os === 'windows' ? <>
            <option value="both">Desktop app + WSL worker</option>
            <option value="desktop">Desktop app only</option>
            <option value="worker">Headless WSL worker only</option>
          </> : <>
            <option value="worker">Worker CLI</option>
            {os === 'linux' && <>
              <option value="central">Networked central server</option>
              <option value="standalone">Standalone central server (local-only)</option>
            </>}
          </>}
        </select>
      </label>
      {(role === 'central' || role === 'standalone') && <label className="block text-xs text-secondary mb-3">Remote memory gateway (optional)
        <input disabled={busy} type="url" className="input w-full mt-1 min-h-11" value={gatewayUrl} onChange={(event) => { setGatewayUrl(event.target.value); setCommand(''); }} placeholder="https://memory.example.com" />
      </label>}
      {os === 'windows' ? <p className="text-xs text-muted mb-3">For remote access, save your token in Central access token above first. The generated command contains that token; use it only on a computer you trust.</p>
        : <p className="text-xs text-muted mb-3">Run in Terminal. The command installs from GitHub and prompts privately for any required token. First installation compiles GAH and can take several minutes.</p>}
      {os === 'macos' && <p className="text-xs text-muted mb-3">macOS installs the worker CLI. Run its loop in Terminal; automatic startup and central server installation are not available yet.</p>}
      {os === 'windows' && role !== 'desktop' && <p className="text-xs text-muted mb-3">Run PowerShell as administrator. First-time WSL setup may require a restart and a Linux user login before you rerun the command. Worker networking currently requires trusted LAN/VPN transport enabled on the central server.</p>}
      <button type="button" onClick={reveal} disabled={busy} className="btn-secondary text-xs px-3 py-1.5 min-h-11 disabled:opacity-50">{busy ? 'Preparing…' : `Reveal ${osName} install command`}</button>
      {command && <div className="mt-3">
        <textarea aria-label={`${osName} install command`} readOnly value={command} rows={5} className="input w-full font-mono text-xs" onFocus={(event) => event.target.select()} />
        <button type="button" className="btn-secondary text-xs px-3 py-1.5 min-h-11 mt-2" onClick={async () => {
          try { await navigator.clipboard.writeText(command); setCopied(true); }
          catch { setError('Clipboard access is unavailable on this connection. Select the command above and copy it manually.'); }
        }}>{copied ? 'Copied' : 'Copy command'}</button>
      </div>}
      {error && <p role="alert" className="mt-3 text-xs text-critical">{error}</p>}
      <p className="text-xs text-muted mt-3">Registered does not mean ready. Authenticate the tools you use {os === 'windows' ? 'inside WSL' : 'on the worker'}, add your repository profile, and check its readiness before dispatching. Claude alone is a valid backend; GitHub repositories use gh and GitLab repositories use glab.</p>
    </section>
  );
}

function GatewaySetupSection() {
  const [settings, setSettings] = useState<GatewaySettingsSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [command, setCommand] = useState<string | null>(null);
  const [revealing, setRevealing] = useState(false);
  const [copied, setCopied] = useState(false);

  const load = () => {
    gahApi
      .getGatewaySettings()
      .then((data) => {
        setSettings(data);
        setError(null);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  };
  useEffect(load, []);
  useWsReconnectRefresh(load);

  if (!settings) {
    return (
      <section className="card-padded max-w-2xl">
        <h3 className="text-sm font-semibold text-primary mb-1">Memory gateway</h3>
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
      <h3 className="text-sm font-semibold text-primary mb-1">Memory gateway</h3>
      <p className="text-xs text-muted mb-3">
        This configures memory access; it does not register a worker. Paste on a new machine (macOS or Linux — on Windows, run this inside WSL) to point it at this node's
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

  const load = () => {
    gahApi
      .getAdminUpdatePending()
      .then(data => { setPending(data); setError(null); setEnabled(true); })
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
  };
  useEffect(load, []);
  useWsReconnectRefresh(load);

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
