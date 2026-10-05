import { useEffect, useRef, useState } from 'react';
import { repositoryCli, type LoginRepairView } from '@git-agent-harness/contracts';
import { loginRepairApi } from '../api/client.js';
import { ExternalAnchor } from './ExternalAnchor.js';

const POLL_MS = 2_000;
const FINISHED = new Set<LoginRepairView['status']>(['succeeded', 'failed', 'expired', 'manual', 'install_required']);

export type RepairableLogin = { node_id: string; node_name?: string; backend: string; provider: string | null; installed?: boolean };

/** "Fix login" for one broken login (#1272). The login runs on the machine
 * that owns the credential; this device sees only the link, the code, and
 * the outcome. The repair key lives in this component's memory only. */
export function LoginRepairPanel({ login }: { login: RepairableLogin }) {
  const [session, setSession] = useState<{ key: string; repair: LoginRepairView } | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [text, setText] = useState('');
  const [copied, setCopied] = useState(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const repair = session?.repair;
  const active = !!repair && !FINISHED.has(repair.status);
  useEffect(() => {
    if (!session || !active) return;
    const timer = window.setTimeout(() => {
      loginRepairApi.view(session.repair.id, session.key)
        .then((next) => { if (alive.current) setSession((current) => current && current.repair.id === next.id ? { ...current, repair: next } : current); })
        .catch((err) => { if (alive.current) setError(err instanceof Error ? err.message : String(err)); });
    }, POLL_MS);
    return () => window.clearTimeout(timer);
  }, [session, active]);

  const start = async () => {
    setBusy(true); setError(''); setText(''); setCopied(false);
    try { setSession(await loginRepairApi.start({ node_id: login.node_id, backend: login.backend, provider: login.provider })); }
    catch (err) { setError(err instanceof Error ? err.message : String(err)); }
    finally { setBusy(false); }
  };
  const submit = async () => {
    if (!session) return;
    setBusy(true); setError('');
    try {
      const next = await loginRepairApi.submit(session.repair.id, session.key, text.trim());
      setText('');
      setSession({ ...session, repair: next });
    } catch (err) { setError(err instanceof Error ? err.message : String(err)); }
    finally { setBusy(false); }
  };
  const cancel = async () => {
    if (!session) return;
    await loginRepairApi.cancel(session.repair.id, session.key).catch(() => undefined);
    setSession(null);
  };
  const where = login.node_name ?? 'that machine';

  const tool = repositoryCli(login.backend);
  if (tool && (repair?.status === 'install_required' || !repair && login.installed === false)) {
    return <div className="space-y-2" aria-label="Repository CLI installation">
      <p className="text-sm text-primary">Install {tool.label} ({login.backend}) on {where} before signing in. GAH uses it to read issues and open pull requests.</p>
      <div className="flex flex-wrap gap-2">
        <ExternalAnchor href={tool.installUrl} className="btn-primary inline-flex min-h-11 items-center text-xs">Install {tool.label}</ExternalAnchor>
        <button type="button" className="btn-secondary min-h-11 text-xs" disabled={busy} onClick={() => void start()}>{busy ? 'Checking…' : 'Check and sign in'}</button>
      </div>
      <p className="text-xs text-secondary">Follow the official installation guide, then check again. Sign-in starts only after the CLI is available.</p>
      {error && <p role="alert" className="text-sm text-critical">{error}</p>}
    </div>;
  }
  if (!repair) {
    return <div className="space-y-1">
      <button type="button" className="btn-secondary min-h-11 text-xs" disabled={busy} onClick={() => void start()}>{busy ? 'Starting…' : 'Fix login'}</button>
      {error && <p role="alert" className="text-sm text-critical">{error}</p>}
    </div>;
  }
  return <div className="space-y-2 rounded-md border border-subtle p-3" aria-label="Login repair">
    {(repair.status === 'starting' || repair.status === 'waiting') && <p role="status" className="text-sm text-secondary">Waiting for {repair.backend} on {where}…</p>}
    {repair.status === 'open_url' && <>
      <p className="text-sm text-primary">Open the sign-in page{repair.code ? ' and enter this code' : ''}. This updates when you finish.</p>
      <ExternalAnchor href={repair.url} className="btn-primary inline-flex min-h-11 items-center text-xs">Open sign-in page</ExternalAnchor>
      {repair.code && <div className="flex flex-wrap items-center gap-2">
        <code className="rounded bg-white/5 px-2 py-1 font-mono text-lg tracking-widest text-primary" aria-label="One-time code">{repair.code}</code>
        <button type="button" className="btn-secondary min-h-11 text-xs" onClick={() => {
          void navigator.clipboard?.writeText(repair.code!).then(() => setCopied(true)).catch(() => undefined);
        }}>{copied ? 'Copied' : 'Copy code'}</button>
      </div>}
    </>}
    {repair.status === 'awaiting_input' && <form className="space-y-2" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      {repair.url && <ExternalAnchor href={repair.url} className="btn-primary inline-flex min-h-11 items-center text-xs">Open sign-in page</ExternalAnchor>}
      <label className="block space-y-1 text-sm text-secondary">
        <span>{repair.prompt}</span>
        <input className="input w-full" type={repair.secret ? 'password' : 'text'} autoComplete="off" spellCheck={false}
          value={text} onChange={(event) => setText(event.target.value)} />
      </label>
      <button type="submit" className="btn-primary min-h-11 text-xs" disabled={busy || !text.trim()}>{busy ? 'Sending…' : repair.secret ? 'Save key' : 'Send code'}</button>
    </form>}
    {repair.status === 'succeeded' && <p role="status" className="text-sm text-good">Logged in again. {where} re-checked the login.</p>}
    {repair.status === 'failed' && <p role="alert" className="text-sm text-critical">{repair.reason}</p>}
    {repair.status === 'expired' && <p role="alert" className="text-sm text-critical">The login timed out after 10 minutes and was stopped.</p>}
    {repair.status === 'manual' && <p className="text-sm text-secondary">{repair.instructions}</p>}
    {error && <p role="alert" className="text-sm text-critical">{error}</p>}
    <div className="flex gap-2">
      {active && <button type="button" className="btn-secondary min-h-11 text-xs" onClick={() => void cancel()}>Cancel</button>}
      {(repair.status === 'failed' || repair.status === 'expired') && <button type="button" className="btn-secondary min-h-11 text-xs" onClick={() => void start()}>Try again</button>}
    </div>
  </div>;
}
