import { useCallback, useEffect, useRef, useState } from 'react';
import type { FleetSnapshot, NodeHealthCheckResult, NodeObservationSnapshot } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import { PageHeader } from '../components/ui/PageHeader.js';
import { AddNodeSection } from './SettingsPage.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { formatUpdatedAge } from '../lib/format.js';

function age(timestamp: string | null | undefined): string {
  const millis = timestamp ? Date.parse(timestamp) : NaN;
  return Number.isFinite(millis) ? formatUpdatedAge(millis) : 'unknown';
}

function observationLabel(observation: NodeObservationSnapshot | undefined): string {
  if (!observation) return 'Unknown — awaiting observation';
  if (observation.state === 'stale' || Date.now() - Date.parse(observation.observed_at) > 120_000) {
    return `Stale — last result: ${observation.state.replaceAll('_', ' ')}`;
  }
  return observation.state === 'healthy' ? 'Healthy' : `Unhealthy — ${observation.state.replaceAll('_', ' ')}`;
}

function Resources({ observation }: { observation?: NodeObservationSnapshot }) {
  const pressure = observation?.resource_pressure;
  return <p className="text-sm text-secondary">
    CPU: {pressure?.cpu_percent == null ? 'unknown' : `${pressure.cpu_percent.toFixed(1)}%`}
    {' · '}Memory: {pressure?.rss_bytes == null ? 'unknown' : `${(pressure.rss_bytes / 1024 / 1024).toFixed(0)} MiB`}
    {' · '}Disk: {pressure?.disk_percent == null ? 'unknown' : `${pressure.disk_percent.toFixed(1)}%`}
  </p>;
}

/** Fleet data comes from the existing liveness scheduler. WebSocket messages
 * only invalidate this authenticated snapshot; they never carry node details.
 */
export function NodesPage() {
  const { messages, reconnectSeq, isConnected, profile } = useWebSocket();
  const [fleet, setFleet] = useState<FleetSnapshot | null>(null);
  const [error, setError] = useState('');
  const [refreshing, setRefreshing] = useState(false);
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [health, setHealth] = useState<NodeHealthCheckResult | null>(null);
  const [healthError, setHealthError] = useState('');
  const [checking, setChecking] = useState(false);
  const [showSetup, setShowSetup] = useState(false);
  const refreshSequence = useRef(0);
  const healthSequence = useRef(0);
  const lastChange = [...messages].reverse().find(({ message }) => message.type === 'fleet.changed')?.id ?? 0;
  const refresh = useCallback(async () => {
    const sequence = ++refreshSequence.current;
    setRefreshing(true);
    try {
      const snapshot = await gahApi.getFleetSnapshot();
      if (sequence !== refreshSequence.current) return;
      setFleet(snapshot); setError(''); setFetchedAt(Date.now());
    } catch (err) {
      if (sequence === refreshSequence.current) setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (sequence === refreshSequence.current) setRefreshing(false);
    }
  }, []);
  useEffect(() => { void refresh(); }, [refresh, lastChange, reconnectSeq]);
  useEffect(() => () => { ++refreshSequence.current; ++healthSequence.current; }, []);

  const check = async (nodeId: string) => {
    const sequence = ++healthSequence.current;
    setSelectedId(nodeId); setHealth(null); setHealthError(''); setChecking(true);
    try {
      const result = await gahApi.checkNodeHealth(nodeId);
      if (sequence !== healthSequence.current) return;
      setHealth(result);
      void refresh();
    } catch (err) {
      if (sequence === healthSequence.current) setHealthError(err instanceof Error ? err.message : String(err));
    } finally {
      if (sequence === healthSequence.current) setChecking(false);
    }
  };
  const selected = fleet?.nodes.find((node) => node.node_id === selectedId);
  const observation = fleet?.observations.find((item) => item.node_id === selectedId) ?? health?.snapshot ?? undefined;
  const leases = fleet?.leases.filter((lease) => lease.node_id === selectedId) ?? [];
  const empty = fleet?.nodes.length === 0;

  return <>
    <PageHeader title="Nodes" description="Registered workers, observed health, and work ownership."
      onRefresh={refresh} refreshing={refreshing} lastUpdated={fetchedAt}
      actions={<button type="button" className="btn-secondary" aria-expanded={showSetup || empty} onClick={() => setShowSetup(!showSetup)}>Register a node</button>} />
    {!isConnected && <p role="status" className="mb-4 text-sm text-warning">Live updates disconnected. Showing the last fetched snapshot; reconnect or refresh to update.</p>}
    {error && <p role="alert" className="mb-4 text-sm text-critical">Cannot load the registry: {error}. Use Refresh to retry.</p>}
    {!fleet && !error && <p role="status" className="text-secondary">Loading registered nodes…</p>}
    {empty && <p className="mb-4 text-secondary">No registered nodes. Install a worker or register an existing worker below.</p>}
    {(showSetup || empty) && <div className="mb-6 space-y-4"><AddNodeSection /><RegisterWorker profile={profile ?? 'gah'} /></div>}
    {fleet && fleet.nodes.length > 0 && <>
      <p className="mb-3 text-sm text-secondary">Observations refresh through the server every 60 seconds. Results older than 2 minutes are marked stale. Select a node to run a live check.</p>
      <div className="divide-y divide-subtle border-y border-subtle">
        {fleet.nodes.map((node) => {
          const observed = fleet.observations.find((item) => item.node_id === node.node_id);
          const label = observationLabel(observed);
          const tone = label.startsWith('Stale') ? 'text-warning' : label.startsWith('Unhealthy') ? 'text-critical' : label === 'Healthy' ? 'text-primary' : 'text-secondary';
          return <section key={node.node_id} className="py-4 space-y-2" aria-label={node.display_name}>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <button type="button" className="text-base font-semibold text-primary underline underline-offset-4" aria-pressed={selectedId === node.node_id} onClick={() => void check(node.node_id)}>{node.display_name}</button>
              <span className={`text-sm ${tone}`}>{label}</span>
            </div>
            <dl className="grid gap-x-6 gap-y-1 text-sm sm:grid-cols-2">
              <div><dt className="inline text-secondary">Node ID: </dt><dd className="inline break-all text-primary">{node.node_id}</dd></div>
              <div><dt className="inline text-secondary">Address: </dt><dd className="inline break-all text-primary">{node.advertised_url}</dd></div>
              <div><dt className="inline text-secondary">Transport: </dt><dd className="inline text-primary">{node.transport_mode}</dd></div>
              <div><dt className="inline text-secondary">Declared profiles: </dt><dd className="inline text-primary">{node.profiles?.join(', ') || 'None — cannot claim work'}</dd></div>
              <div><dt className="inline text-secondary">Last seen: </dt><dd className="inline text-primary">{age(observed?.last_seen_at ?? node.last_seen_at)}</dd></div>
              <div><dt className="inline text-secondary">Observed: </dt><dd className="inline text-primary">{age(observed?.observed_at)}</dd></div>
            </dl>
            {(observed?.error || node.last_error_kind) && <p className="text-sm text-critical">{observed?.error?.kind ?? node.last_error_kind}: {observed?.error?.message ?? node.last_error_message}</p>}
            <Resources observation={observed} />
          </section>;
        })}
      </div>
    </>}
    {selected && <section className="mt-6 space-y-3" aria-label="Node detail">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-base font-semibold text-primary">{selected.display_name}: health and work</h3>
        <button type="button" className="btn-secondary" disabled={checking} onClick={() => void check(selected.node_id)}>Check health</button>
      </div>
      {checking && <p role="status" className="text-secondary">Checking node health…</p>}
      {healthError && <p role="alert" className="text-critical">Live check failed: {healthError}</p>}
      {health && <div className="space-y-2">
        <p className="text-sm text-primary">Last manual check: {health.status} · {health.state.replaceAll('_', ' ')} · checked {age(new Date(health.timestamp).toISOString())}</p>
        {health.error && <p role="alert" className="text-critical">{health.error.kind}: {health.error.message}</p>}
        <p className="text-sm text-secondary">Resources at that manual check:</p>
        <Resources observation={health.snapshot ?? undefined} />
      </div>}
      <h4 className="font-medium text-primary">Observed local claims</h4>
      <p className="text-sm text-secondary">{observationLabel(observation)} · observed {age(observation?.observed_at)}{observation?.profile ? ` · profile ${observation.profile}` : ''}</p>
      {!observation || (observation.state !== 'healthy' && observation.state !== 'stale') ? <p className="text-sm text-secondary">Local claims are unknown until the node returns a status snapshot.</p>
        : observation.active_claims.length === 0 ? <p className="text-sm text-secondary">No local claims reported in this observation.</p>
        : <ul className="space-y-1 text-sm text-primary">{observation.active_claims.map((claim) => <li key={`${claim.work_id}:${claim.pid}`}>{claim.work_id} · {claim.scope} · PID {claim.pid} · claimed {age(claim.claimed_at)}</li>)}</ul>}
      <h4 className="font-medium text-primary">Central leases</h4>
      {leases.length === 0 ? <p className="text-sm text-secondary">No active central leases at the last refresh.</p>
        : <ul className="space-y-2 text-sm text-primary">{leases.map((lease) => <li key={`${lease.profile}:${lease.work_id}`}>{lease.profile} / {lease.work_id} · renewed {age(lease.renewed_at)} · expires {new Date(lease.expires_at).toLocaleString()}</li>)}</ul>}
    </section>}
  </>;
}

/** Produce an executable command from explicit worker connection settings.
 * Credentials stay in the worker environment and central secret reference.
 */
function RegisterWorker({ profile }: { profile: string }) {
  const [central, setCentral] = useState(window.location.origin);
  const [self, setSelf] = useState('http://127.0.0.1:3773');
  const [profiles, setProfiles] = useState(profile);
  const [transport, setTransport] = useState('trusted_lan');
  const [secret, setSecret] = useState('env:GAH_NODE_TOKEN');
  const [command, setCommand] = useState('');
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState('');
  const fieldClass = 'w-full mt-1 rounded-md border border-subtle bg-raised px-3 py-2 text-base text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent';
  return <form className="card-padded max-w-2xl space-y-3" onChange={() => { setCommand(''); setCopied(false); }} onSubmit={(event) => {
    event.preventDefault(); setError('');
    try {
      for (const address of [central, self]) {
        const url = new URL(address);
        if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error('Use an HTTP or HTTPS address without embedded credentials.');
      }
      if (!profiles.split(',').some((name) => name.trim())) throw new Error('Enter at least one worker profile.');
      if (!/^(env:|file:).+/.test(secret)) throw new Error('Use an env:NAME or file:/path secret reference on the central server.');
      const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
      setCommand(`npm run register-node --workspace=apps/server -- --central-url ${quote(central)} --self-url ${quote(self)} --transport-mode ${quote(transport)} --secret-ref ${quote(secret)} --profiles ${quote(profiles)}`);
    } catch (err) { setError(err instanceof Error ? err.message : String(err)); }
  }}>
    <h3 className="text-base font-semibold text-primary">Register an existing worker</h3>
    <p className="text-sm text-secondary">Run this from the GAH checkout on macOS, Linux, or WSL with its node server running. Set COORDINATOR_TOKEN in that shell to the central access token. The worker identity must advertise an address reachable from central.</p>
    <label className="block text-sm text-secondary">Central address<input required type="url" className={fieldClass} value={central} onChange={(event) => setCentral(event.target.value)} /></label>
    <label className="block text-sm text-secondary">Worker’s local server address<input required type="url" className={fieldClass} value={self} onChange={(event) => setSelf(event.target.value)} /></label>
    <label className="block text-sm text-secondary">Profiles (comma-separated)<input required className={fieldClass} value={profiles} onChange={(event) => setProfiles(event.target.value)} /></label>
    <label className="block text-sm text-secondary">Transport<select className={fieldClass} value={transport} onChange={(event) => setTransport(event.target.value)}><option value="trusted_lan">Trusted LAN / VPN</option><option value="authenticated_remote">Authenticated remote (HTTPS)</option><option value="loopback">Loopback on central</option></select></label>
    <label className="block text-sm text-secondary">Worker token reference on central<input required className={fieldClass} value={secret} onChange={(event) => setSecret(event.target.value)} /></label>
    <p className="text-sm text-secondary">Central must resolve this reference to the worker’s access token. Trusted LAN HTTP requires GAH_ALLOW_INSECURE_HTTP=1 on central and worker.</p>
    <button type="submit" className="btn-secondary">Generate register-node command</button>
    {command && <div className="space-y-2"><textarea aria-label="Register node command" readOnly className={`${fieldClass} font-mono text-sm`} rows={5} value={command} onFocus={(event) => event.target.select()} />
      <button type="button" className="btn-secondary" onClick={async () => { try { await navigator.clipboard.writeText(command); setCopied(true); } catch { setError('Clipboard unavailable. Select and copy the command manually.'); } }}>{copied ? 'Copied' : 'Copy register-node command'}</button></div>}
    {error && <p role="alert" className="text-critical">{error}</p>}
  </form>;
}
