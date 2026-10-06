import { useState } from 'react';
import { Loader2, Play } from 'lucide-react';
import type { AvailableTicket, ProfileSummary, Session } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import { useBackendModels } from './AgentLimitsSection.js';
import { AgentModelSelect } from './AgentModelSelect.js';
import { agentDisplayName } from './LiveAgentsCard.js';
import { currentAgentModelLabel } from '../lib/agentModelLabel.js';

const INPUT = 'w-full min-h-11 rounded-md border border-subtle bg-raised px-3 py-2 text-sm text-primary';
const EFFORT_NAMES: Record<string, string> = { low: 'Low', medium: 'Medium', high: 'High', xhigh: 'Extra high', max: 'Maximum', ultra: 'Ultra' };

export function StartWorkerSection({ profile, agents, tickets, sessions, onActivity }: {
  profile: ProfileSummary;
  agents: string[];
  tickets: AvailableTicket[];
  sessions: Session[];
  onActivity: () => void;
}) {
  const backends = [...new Set(agents.map((agent) => agent.split('/')[0]))].sort();
  const catalogs = useBackendModels(profile.name, backends);
  const [subscriber, setSubscriber] = useState('');
  const backend = backends.includes(subscriber) ? subscriber : backends[0] ?? '';
  const [models, setModels] = useState<Record<string, string>>({});
  const [efforts, setEfforts] = useState<Record<string, string>>({});
  const catalog = catalogs[backend];
  const model = models[backend] ?? agents.find((agent) => agent.startsWith(`${backend}/`))?.slice(backend.length + 1) ?? catalog?.options[0]?.value ?? '';
  const group = catalog?.options.find((option) => option.variants?.some((variant) => variant.value === model));
  const native = backend === 'codex' || backend === 'claude';
  const reasoning = group?.variants?.map((variant) => ({ id: variant.effort, name: EFFORT_NAMES[variant.effort] ?? variant.effort }))
    ?? (native ? [{ id: 'default', name: 'Provider default' }, ...(catalog?.efforts ?? []).filter((effort) => effort.id !== 'default').map((effort) => ({ ...effort, name: EFFORT_NAMES[effort.id] ?? effort.name }))] : []);
  const effort = group?.variants?.find((variant) => variant.value === model)?.effort ?? efforts[backend] ?? profile.agent_reasoning_effort?.[backend] ?? 'default';
  const available = tickets.filter((ticket) => !ticket.has_active_claim && !ticket.has_active_mr && !ticket.human_required && ticket.execution_policy.dispatchable_now
    && !sessions.some((session) => session.target === ticket.ticket_path && (session.status === 'starting' || session.status === 'running')));
  const [target, setTarget] = useState('');
  const job = available.find((ticket) => ticket.ticket_path === target) ?? available[0];
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [launched, setLaunched] = useState<Session | null>(null);
  const current = sessions.find((session) => session.id === launched?.id) ?? launched;
  const launch = async () => {
    if (!job || !backend || !model.trim()) return;
    setPending(true);
    setError(null);
    setLaunched(null);
    try {
      const result = await gahApi.startWorker({ profile: profile.name, providerKind: profile.provider, repo: profile.repo,
        backend, model, target: job.ticket_path, ...(native ? { reasoningEffort: effort } : {}), requestId: crypto.randomUUID() });
      setLaunched(result.session);
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setPending(false); }
  };
  return <section className="card-padded" aria-labelledby="start-worker-title">
    <h3 id="start-worker-title" className="text-sm font-semibold text-primary">Start an extra worker</h3>
    <p className="mt-1 mb-4 text-xs text-muted">Launch a queued job now with the subscriber and model you choose. Manual starts skip automatic CPU, memory, and worker limits.</p>
    <div className="space-y-3">
      <label className="block space-y-1 text-xs text-secondary">Subscriber
        <select aria-label="Worker subscriber" className={INPUT} value={backend} onChange={(event) => setSubscriber(event.target.value)}>
          {backends.map((backend) => <option key={backend} value={backend}>{agentDisplayName(backend)}</option>)}
        </select>
      </label>
      <label className="block space-y-1 text-xs text-secondary">Model
        <AgentModelSelect label="Worker model" value={model} options={catalog?.options ?? []}
          currentLabel={currentAgentModelLabel(backend, model, catalog?.options ?? [])} loading={catalog?.loading} failed={catalog?.failed}
          onChange={(value) => setModels((current) => ({ ...current, [backend]: value }))} />
      </label>
      <label className="block space-y-1 text-xs text-secondary">Reasoning level
        <select aria-label="Worker reasoning level" className={INPUT} value={effort} disabled={reasoning.length === 0}
          onChange={(event) => {
            if (group) {
              const variant = group.variants?.find((variant) => variant.effort === event.target.value);
              if (variant) setModels((current) => ({ ...current, [backend]: variant.value }));
            } else setEfforts((current) => ({ ...current, [backend]: event.target.value }));
          }}>
          {reasoning.length > 0 && !reasoning.some((choice) => choice.id === effort) && <option value={effort}>{effort} (current)</option>}
          {reasoning.length ? reasoning.map((choice) => <option key={choice.id} value={choice.id}>{choice.name}</option>) : <option value="default">Provider default</option>}
        </select>
        {native && <p className="text-xs text-muted">For this worker only. Saved agent settings stay unchanged.</p>}
      </label>
    </div>
    <label className="mt-4 block space-y-1 text-xs text-secondary">Queued job
      <select aria-label="Worker job" value={available.some((ticket) => ticket.ticket_path === target) ? target : ''} className={INPUT} disabled={!available.length} onChange={(event) => setTarget(event.target.value)}>
        <option value="">Next available job</option>
        {available.map((ticket) => <option key={ticket.ticket_path} value={ticket.ticket_path}>{ticket.title ?? ticket.work_id ?? ticket.ticket_path}</option>)}
      </select>
    </label>
    {job ? <div className="mt-3 rounded-md border border-subtle bg-raised p-3">
      <p className="text-xs text-muted">{target && available.some((ticket) => ticket.ticket_path === target) ? 'Selected job' : 'Next queued job'}</p>
      <p className="mt-1 break-words text-sm text-primary">{job.title ?? job.work_id ?? job.ticket_path}</p>
    </div> : <p role="status" className="mt-4 text-sm text-muted">No eligible queued job is available. Jobs already claimed by a worker are excluded.</p>}
    <button type="button" onClick={() => void launch()} disabled={pending || !job || !backend || !model.trim()}
      className="btn-primary mt-4 inline-flex min-h-11 items-center gap-2 disabled:opacity-50 disabled:cursor-not-allowed">
      {pending ? <Loader2 size={15} className="animate-spin" /> : <Play size={15} />} {pending ? 'Starting worker…' : 'Start worker'}
    </button>
    {error && <p role="alert" className="mt-3 text-sm text-critical">Worker could not start: {error}</p>}
    {current && <div className="mt-3 rounded-md border border-subtle p-3" role={current.status === 'error' ? 'alert' : 'status'}>
      <p className={`text-sm ${current.status === 'error' ? 'text-critical' : 'text-primary'}`}>
        {current.status === 'error' ? `Worker failed: ${current.error ?? 'Open activity for details.'}` : current.status === 'stopped' ? 'Worker finished.' : 'Worker launch started. Follow its live progress and any startup errors in activity.'}
      </p>
      <button type="button" className="mt-2 text-xs text-accent underline" onClick={onActivity}>View live activity</button>
    </div>}
  </section>;
}
