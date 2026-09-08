import { useId } from 'react';
import type { ChatNodeInfo } from '@git-agent-harness/contracts';

/** Selects a node for the next turn and explains unavailable choices. */
export function ChatNodePicker({ nodes, value, onChange, loading, error, retry, disabled = false }: {
  nodes: ChatNodeInfo[]; value: string; onChange: (nodeId: string) => void;
  loading: boolean; error: string | null; retry: () => void; disabled?: boolean;
}) {
  const id = useId();
  const selected = nodes.find(node => node.nodeId === value);
  const observedAt = selected?.observedAt ? Date.parse(selected.observedAt) : NaN;
  return <div className="min-w-0 space-y-1 text-sm">
    <label htmlFor={id} className="block text-secondary">Run on node</label>
    <select id={id} value={value} onChange={event => onChange(event.target.value)}
      disabled={disabled || loading || !!error || nodes.length === 0}
      aria-describedby={`${id}-status`}
      className="min-h-11 w-full min-w-0 rounded-md border border-subtle bg-raised px-2 py-1.5 text-base text-primary disabled:opacity-60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
      {!selected && <option value={value}>{loading ? 'Loading nodes…' : value ? 'Selected node unavailable' : 'Select a node'}</option>}
      {nodes.map(node => <option key={node.nodeId} value={node.nodeId} disabled={!(node.eligible ?? node.chatCapable)}>
        {node.displayName} · {node.role}{!(node.eligible ?? node.chatCapable) ? ` · ${node.reason ?? 'Unavailable'}` : ''}
      </option>)}
    </select>
    <div id={`${id}-status`} className="text-sm text-secondary" role="status">
      {error ? <>{error} <button type="button" className="min-h-11 px-2 underline hover:text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent" onClick={retry}>Retry nodes</button></>
        : loading ? 'Checking cached node readiness…'
        : selected && !(selected.eligible ?? selected.chatCapable) ? selected.reason ?? 'This node cannot run the selected project and provider.'
        : selected ? `${selected.state?.replaceAll('_', ' ') ?? 'Available'}${Number.isFinite(observedAt) ? ` · observed ${new Date(observedAt).toLocaleString()}` : ''}`
        : nodes.length === 0 ? 'No nodes are available for this project.' : 'Choose an available node.'}
    </div>
  </div>;
}
