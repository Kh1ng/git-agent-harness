import { useEffect } from 'react';
import { RefreshCw } from 'lucide-react';
import { useGahStore } from '../store/gahStore.js';
import { useUiStore } from '../store/uiStore.js';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { StatusBadge } from './ui/StatusBadge.js';

/** This node's own readiness for the selected profile: config, provider
 * authentication, filesystem, and backend executables. Worker readiness is
 * checked per worker on the Fleet page. */
export function NodeReadinessCard() {
  const { profile } = useWebSocket();
  const profileOverride = useUiStore((s) => s.profileOverride);
  const doctor = useGahStore((s) => s.doctor);
  const fetchDoctor = useGahStore((s) => s.fetchDoctor);
  const selectedName = profileOverride ?? profile ?? '';
  useEffect(() => {
    if (selectedName) fetchDoctor(selectedName);
  }, [selectedName, fetchDoctor]);

  return (
    <section className="card-padded max-w-3xl mb-6" aria-labelledby="node-readiness-title">
        <div className="flex items-start justify-between gap-3 mb-3">
          <div>
            <h3 id="node-readiness-title" className="text-sm font-semibold text-primary">Node readiness</h3>
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
  );
}
