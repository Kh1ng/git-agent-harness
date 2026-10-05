import { useEffect, useRef, useState } from 'react';
import { Loader2, Save, Zap, X } from 'lucide-react';
import { useGahStore } from '../store/gahStore.js';
import type { ProfileSummary } from '@git-agent-harness/contracts';

interface WorkerScalingSectionProps {
  selectedName: string;
  selected: Pick<ProfileSummary, 'max_parallel_workers' | 'worker_scaling'>;
}

const INPUT_CLASS = 'w-full bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary';
const BUTTON_CLASS = 'inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-sm font-medium disabled:opacity-50 disabled:cursor-not-allowed';

/** A whole number at or above `min`, or undefined for blank or invalid text. */
function wholeNumber(text: string, min: number): number | undefined {
  const value = Number(text.trim());
  return text.trim() !== '' && Number.isInteger(value) && value >= min ? value : undefined;
}

/**
 * Worker count beyond the profile baseline: automatic scaling while a
 * subscription has quota headroom, and a manual boost. Both are saved to the
 * profile and picked up by the loop on its next iteration.
 */
export function WorkerScalingSection({ selectedName, selected }: WorkerScalingSectionProps) {
  const updateProfile = useGahStore((s) => s.updateProfile);
  const saving = useGahStore((s) => s.profileCrud.updating);
  const saveError = useGahStore((s) => s.profileCrud.updateError);
  const status = useGahStore((s) => s.status);
  const fetchStatus = useGahStore((s) => s.fetchStatus);

  const scaling = selected.worker_scaling;
  const baseline = selected.max_parallel_workers ?? 1;

  const [enabled, setEnabled] = useState(false);
  const [maxWorkers, setMaxWorkers] = useState('');
  const [extraPerModel, setExtraPerModel] = useState('1');
  const [minRemaining, setMinRemaining] = useState('50');
  const [boostWorkers, setBoostWorkers] = useState('1');
  const [boostModel, setBoostModel] = useState('');
  const [boostHours, setBoostHours] = useState('');
  // Seed once per profile so a refresh after saving keeps in-progress edits.
  const seededProfileRef = useRef<string | null>(null);

  useEffect(() => {
    if (seededProfileRef.current === selectedName) return;
    seededProfileRef.current = selectedName;
    setEnabled(scaling?.enabled ?? false);
    setMaxWorkers(scaling?.max_workers != null ? String(scaling.max_workers) : '');
    setExtraPerModel(String(scaling?.extra_per_model ?? 1));
    setMinRemaining(String(scaling?.min_remaining_percent ?? 50));
  }, [selectedName, scaling]);

  useEffect(() => {
    void fetchStatus(selectedName);
  }, [selectedName, fetchStatus]);

  if (!scaling) {
    return (
      <section className="card-padded max-w-md">
        <h3 className="text-sm font-semibold text-primary mb-1">Worker scaling</h3>
        <p className="text-xs text-muted">This node's <code>gah</code> does not support worker scaling yet. Update it to use these settings.</p>
      </section>
    );
  }

  const limits = status.key === selectedName ? status.data?.worker_limits : undefined;
  const maxWorkersValue = wholeNumber(maxWorkers, 1);
  const extraValue = wholeNumber(extraPerModel, 0);
  const minRemainingValue = Number(minRemaining);
  const scalingError = maxWorkers.trim() !== '' && maxWorkersValue === undefined
    ? 'Most workers must be a whole number of at least 1.'
    : extraValue === undefined
      ? 'Extra runs per agent must be a whole number.'
      : minRemaining.trim() === '' || !(minRemainingValue >= 0 && minRemainingValue <= 100)
        ? 'Usage left must be between 0 and 100.'
        : null;
  const boostWorkersValue = wholeNumber(boostWorkers, 1);
  const boostHoursValue = Number(boostHours);
  const boostError = boostWorkersValue === undefined
    ? 'Workers to add must be a whole number of at least 1.'
    : boostHours.trim() !== '' && !(boostHoursValue > 0)
      ? 'Hours must be greater than zero.'
      : null;

  const save = async (data: Parameters<typeof updateProfile>[1]) => {
    await updateProfile(selectedName, data);
    await fetchStatus(selectedName, { force: true });
  };
  const saveScaling = () => save({
    worker_scaling: enabled ? 'on' : 'off',
    worker_scaling_extra_per_model: extraValue,
    worker_scaling_min_remaining_percent: minRemainingValue,
    ...(maxWorkersValue !== undefined
      ? { worker_scaling_max_workers: maxWorkersValue }
      : { clear: ['worker_scaling_max_workers'] }),
  });
  const startBoost = () => save({
    boost_workers: boostWorkersValue,
    ...(boostModel.trim() ? { boost_model: boostModel.trim() } : {}),
    ...(boostHours.trim() ? { boost_hours: boostHoursValue } : {}),
  });
  const endBoost = () => save({ clear: ['worker_boost'] });

  const boostActive = (scaling.boost_workers ?? 0) > 0;

  return (
    <section className="card-padded max-w-md">
      <h3 className="text-sm font-semibold text-primary mb-1">Worker scaling</h3>
      <p className="text-xs text-muted mb-3">
        Workers beyond the baseline of <span className="font-mono text-secondary">{baseline}</span> for{' '}
        <span className="font-mono text-secondary">{selectedName}</span>. Free memory and CPU still decide
        whether an extra worker actually starts.
      </p>

      {limits && (
        <div className="mb-3 rounded-md border border-subtle bg-raised px-3 py-2" role="status">
          <p className="text-sm text-primary">
            Now: <span className="font-semibold">{limits.workers}</span> workers
            {limits.workers !== limits.baseline_workers && <> (baseline {limits.baseline_workers})</>}
          </p>
          {limits.notes?.map((note) => (
            <p key={note} className="text-xs text-muted mt-0.5">{note}</p>
          ))}
        </div>
      )}

      <div className="space-y-3">
        <label className="flex items-start gap-2 text-sm text-primary">
          <input type="checkbox" className="mt-0.5" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
          <span>
            Scale up automatically
            <span className="block text-xs text-muted">
              An agent runs extra copies while every usage window of its subscription, the 5-hour one
              included, has enough left. An agent with no recent usage reading is never scaled.
            </span>
          </span>
        </label>

        <div>
          <label htmlFor="worker-scaling-min-remaining" className="block text-xs font-medium text-secondary mb-1">Usage left needed to scale (%)</label>
          <input id="worker-scaling-min-remaining" type="number" min={0} max={100} value={minRemaining} onChange={(e) => setMinRemaining(e.target.value)} className={INPUT_CLASS} />
        </div>
        <div>
          <label htmlFor="worker-scaling-extra" className="block text-xs font-medium text-secondary mb-1">Extra runs per agent</label>
          <input id="worker-scaling-extra" type="number" min={0} value={extraPerModel} onChange={(e) => setExtraPerModel(e.target.value)} className={INPUT_CLASS} />
        </div>
        <div>
          <label htmlFor="worker-scaling-max" className="block text-xs font-medium text-secondary mb-1">Most workers when scaled</label>
          <input id="worker-scaling-max" type="number" min={1} value={maxWorkers} onChange={(e) => setMaxWorkers(e.target.value)} placeholder={String(baseline * 2)} className={INPUT_CLASS} />
          <p className="text-xs text-muted mt-1">Blank means twice the baseline.</p>
        </div>
      </div>

      {scalingError && <p role="alert" className="mt-3 text-xs text-critical">{scalingError}</p>}
      <button onClick={saveScaling} disabled={saving || scalingError != null} className={`mt-3 bg-accent text-white hover:bg-accent/90 ${BUTTON_CLASS}`}>
        {saving ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <Save size={14} aria-hidden="true" />}
        Save scaling settings
      </button>

      <h4 className="text-sm font-semibold text-primary mt-5 mb-1">Add workers now</h4>
      {boostActive ? (
        <div className="flex items-center justify-between gap-2 rounded-md border border-subtle bg-raised px-3 py-2">
          <p className="text-sm text-primary">
            +{scaling.boost_workers} for {scaling.boost_model ?? 'every capped agent'}
            <span className="block text-xs text-muted">
              {scaling.boost_until ? `Until ${new Date(scaling.boost_until).toLocaleString()}` : 'Until you end it'}
            </span>
          </p>
          <button onClick={endBoost} disabled={saving} className={`bg-raised border border-subtle text-secondary hover:bg-white/5 ${BUTTON_CLASS}`}>
            <X size={14} aria-hidden="true" />
            End boost
          </button>
        </div>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label htmlFor="worker-boost-count" className="block text-xs font-medium text-secondary mb-1">Workers to add</label>
              <input id="worker-boost-count" type="number" min={1} value={boostWorkers} onChange={(e) => setBoostWorkers(e.target.value)} className={INPUT_CLASS} />
            </div>
            <div>
              <label htmlFor="worker-boost-hours" className="block text-xs font-medium text-secondary mb-1">For how many hours</label>
              <input id="worker-boost-hours" type="number" min={0} step="any" value={boostHours} onChange={(e) => setBoostHours(e.target.value)} placeholder="Until ended" className={INPUT_CLASS} />
            </div>
          </div>
          <div className="mt-3">
            <label htmlFor="worker-boost-model" className="block text-xs font-medium text-secondary mb-1">Agent</label>
            <input id="worker-boost-model" type="text" value={boostModel} onChange={(e) => setBoostModel(e.target.value)} placeholder="Every capped agent" className={INPUT_CLASS} />
            <p className="text-xs text-muted mt-1">
              Written as <code>backend/model</code>, for example <code>codex/gpt-5</code>. A boost is not
              limited by the scaled maximum above.
            </p>
          </div>
          {boostError && <p role="alert" className="mt-3 text-xs text-critical">{boostError}</p>}
          <button onClick={startBoost} disabled={saving || boostError != null} className={`mt-3 bg-accent text-white hover:bg-accent/90 ${BUTTON_CLASS}`}>
            <Zap size={14} aria-hidden="true" />
            Add workers
          </button>
        </>
      )}

      {saveError && <p className="mt-3 text-xs text-critical">Failed to save: {saveError}</p>}
    </section>
  );
}
