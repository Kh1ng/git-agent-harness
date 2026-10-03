export function MistralConnectionPanel() {
  const native = window.__GAH_DESKTOP_MISTRAL_LOGIN__ === true;
  return (
    <section className="card-padded" aria-labelledby="mistral-connection-heading">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h3 id="mistral-connection-heading" className="text-sm font-semibold text-primary">Connect Mistral</h3>
          <p className="text-xs text-muted mt-1">{native
            ? 'Sign in from this computer’s Settings. Registered workers share usage readings with central.'
            : 'Open GAH’s desktop app on a computer with a worker, then use Settings → Mistral account usage.'}</p>
        </div>
        {native && <a href="gah://settings" className="btn-secondary text-xs min-h-11">Open this computer’s Settings</a>}
      </div>
    </section>
  );
}
