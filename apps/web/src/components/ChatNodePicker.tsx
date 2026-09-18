import { useId, type ChangeEvent } from 'react';
import { Server } from 'lucide-react';
import type { ChatNodeInfo } from '@git-agent-harness/contracts';

/** Selects a node for the next turn and explains unavailable choices.
 * `compact` renders a composer pill; the explanation stays in the tooltip
 * and only surfaces inline when something is wrong. */
export function ChatNodePicker({ nodes, value, onChange, loading, error, retry, disabled = false, compact = false }: {
  nodes: ChatNodeInfo[]; value: string; onChange: (nodeId: string) => void;
  loading: boolean; error: string | null; retry: () => void; disabled?: boolean; compact?: boolean;
}) {
  const id = useId();
  const selected = nodes.find(node => node.nodeId === value);
  const observedAt = selected?.observedAt ? Date.parse(selected.observedAt) : NaN;
  const unavailable = !!selected && !(selected.eligible ?? selected.chatCapable);
  const status = error ? error
    : loading ? 'Checking cached node readiness…'
    : unavailable ? selected.reason ?? 'This node cannot run the selected project and provider.'
    : selected ? `${selected.state?.replaceAll('_', ' ') ?? 'Available'}${Number.isFinite(observedAt) ? ` · observed ${new Date(observedAt).toLocaleString()}` : ''}`
    : nodes.length === 0 ? 'No nodes are available for this project.' : 'Choose an available node.';
  const retryButton = <button type="button" className="min-h-11 px-2 underline hover:text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent" onClick={retry}>Retry nodes</button>;
  const options = <>
    {!selected && <option value={value}>{loading ? 'Loading nodes…' : value ? 'Selected node unavailable' : 'Select a node'}</option>}
    {nodes.map(node => <option key={node.nodeId} value={node.nodeId} disabled={!(node.eligible ?? node.chatCapable)}>
      {node.displayName} · {node.role}{!(node.eligible ?? node.chatCapable) ? ` · ${node.reason ?? 'Unavailable'}` : ''}
    </option>)}
  </>;
  const selectProps = {
    id, value, onChange: (event: ChangeEvent<HTMLSelectElement>) => onChange(event.target.value),
    disabled: disabled || loading || !!error || nodes.length === 0, 'aria-describedby': `${id}-status`
  };

  if (compact) {
    const problem = !!error || unavailable || (!loading && !selected);
    return <span className="inline-flex min-w-0 items-center gap-1 text-xs">
      <label htmlFor={id} className="relative inline-flex min-w-0 items-center" title={`Run on node · ${status}. Each node uses its own checkout; files do not move.`}>
        <Server size={13} className={`pointer-events-none absolute left-2 ${problem ? 'text-warning' : 'text-muted'}`} aria-hidden="true" />
        <select {...selectProps} aria-label="Run on node"
          className="max-w-[11rem] max-sm:min-h-11 max-sm:min-w-11 max-sm:max-w-[5.5rem] min-w-0 truncate rounded-md border border-subtle bg-raised py-2 pl-7 pr-2 text-xs text-secondary hover:bg-white/5 disabled:opacity-60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
          {options}
        </select>
      </label>
      <span id={`${id}-status`} role="status" className={problem ? 'text-warning' : 'sr-only'}>
        {problem && status}{error && retryButton}
      </span>
    </span>;
  }

  return <div className="min-w-0 space-y-1 text-sm">
    <label htmlFor={id} className="block text-secondary">Run on node</label>
    <select {...selectProps}
      className="min-h-11 w-full min-w-0 rounded-md border border-subtle bg-raised px-2 py-1.5 text-base text-primary disabled:opacity-60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
      {options}
    </select>
    <div id={`${id}-status`} className="text-sm text-secondary" role="status">
      {status}{error && <> {retryButton}</>}
    </div>
  </div>;
}
