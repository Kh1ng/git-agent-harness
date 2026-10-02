import type { PlanningMap, PlanningNode, PlanningNodeState } from '@git-agent-harness/contracts';
import { nodeName } from '../lib/planningTarget.js';
import { STAR_MAP_SIZE, layoutStarMap } from '../lib/starMapLayout.js';

/** Fill colour per state; frontier stars also glow. */
export const STATE_CLASS: Record<PlanningNodeState, string> = {
  ready: 'text-accent',
  blocked: 'text-warning',
  parent: 'text-secondary',
  done: 'text-muted',
  ruled_out: 'text-muted',
  claimed: 'text-series-5'
};

export const STATE_LABEL: Record<PlanningNodeState, string> = {
  ready: 'Ready',
  blocked: 'Blocked',
  parent: 'In progress below',
  done: 'Done',
  ruled_out: 'Ruled out',
  claimed: 'Claimed'
};

/** How solid a star is: done work fades; ruled-out work is only an outline. */
const FILL_OPACITY: Partial<Record<PlanningNodeState, number>> = { done: 0.45, ruled_out: 0 };

/**
 * An epic as a star map: the epic at the centre, its issues on rings by
 * depth, blocker links as arrows. Ready work glows. Read-only: selecting a
 * star only reports it.
 */
export function StarMap({ map, selected, onSelect }: {
  map: PlanningMap;
  selected: number | null;
  onSelect: (node: PlanningNode) => void;
}) {
  const positions = layoutStarMap(map);
  const byNumber = new Map(map.nodes.map((node) => [node.number, node]));
  const maxDepth = Math.max(0, ...map.nodes.map((node) => node.depth ?? 0));
  const rings = maxDepth + (map.nodes.some((node) => node.depth === null) ? 1 : 0);
  const centre = STAR_MAP_SIZE / 2;
  const ringStep = rings === 0 ? 0 : (STAR_MAP_SIZE / 2 - 48) / rings;

  return (
    <svg
      viewBox={`0 0 ${STAR_MAP_SIZE} ${STAR_MAP_SIZE}`}
      className="h-auto w-full max-h-[70vh]"
      role="group"
      aria-label={map.file ? `Planning map ${map.file}` : `Planning map for epic #${map.epic}`}
    >
      <defs>
        <marker id="star-map-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M0,0 L10,5 L0,10 z" className="text-warning" fill="currentColor" />
        </marker>
      </defs>
      {Array.from({ length: rings }, (_, index) => (
        <circle key={index} cx={centre} cy={centre} r={(index + 1) * ringStep}
          className="text-muted" fill="none" stroke="currentColor" strokeOpacity={0.25} strokeDasharray="2 6" strokeWidth={1} />
      ))}
      {map.edges.map((edge) => {
        const from = positions.get(edge.from);
        const to = positions.get(edge.to);
        if (!from || !to) return null;
        const blocker = byNumber.get(edge.from);
        const settled = edge.kind === 'blocks' && blocker?.state === 'done';
        // Stop arrows short of the star they point at.
        const length = Math.hypot(to.x - from.x, to.y - from.y) || 1;
        const trim = edge.kind === 'blocks' ? 12 : 0;
        const end = { x: to.x - ((to.x - from.x) * trim) / length, y: to.y - ((to.y - from.y) * trim) / length };
        return (
          <line key={`${edge.kind}-${edge.from}-${edge.to}`} x1={from.x} y1={from.y} x2={end.x} y2={end.y}
            className={edge.kind === 'child' || settled ? 'text-muted' : 'text-warning'}
            stroke="currentColor"
            strokeOpacity={edge.kind === 'child' ? 0.35 : settled ? 0.4 : 0.9}
            strokeWidth={edge.kind === 'child' ? 1 : 1.5}
            strokeDasharray={settled ? '4 4' : undefined}
            markerEnd={edge.kind === 'blocks' && !settled ? 'url(#star-map-arrow)' : undefined} />
        );
      })}
      {map.nodes.map((node) => {
        const at = positions.get(node.number);
        if (!at) return null;
        const epic = node.number === map.epic;
        const radius = epic ? 15 : 9;
        const isSelected = node.number === selected;
        // A map file's centre is its destination, not a ticket.
        const name = map.file && epic ? 'map' : nodeName(map, node.number);
        const hollow = node.state === 'ruled_out';
        return (
          <g key={node.number} role="button" tabIndex={0}
            aria-label={`${map.file && epic ? 'Map' : name} ${node.title}: ${STATE_LABEL[node.state]}${node.depth === null ? ', outside the epic' : ''}`}
            aria-pressed={isSelected}
            className={`${STATE_CLASS[node.state]} cursor-pointer`}
            onClick={() => onSelect(node)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); onSelect(node); }
            }}>
            <title>{`#${node.number} ${node.title}`}</title>
            {/* Hit area: the star and its label, with no gap between them. */}
            <rect x={at.x - 20} y={at.y - radius - 6} width={40} height={radius * 2 + 26} fill="transparent" />
            {node.state === 'ready' && !epic && <circle cx={at.x} cy={at.y} r={radius + 7} fill="currentColor" opacity={0.18} />}
            <circle cx={at.x} cy={at.y} r={radius} fill="currentColor"
              fillOpacity={FILL_OPACITY[node.state] ?? 1}
              stroke={isSelected ? 'rgb(var(--ink-primary))' : node.depth === null || hollow ? 'currentColor' : 'none'}
              strokeWidth={isSelected ? 2.5 : 1.5}
              strokeDasharray={node.depth === null && !isSelected ? '3 3' : undefined} />
            <text x={at.x} y={at.y + radius + 14} textAnchor="middle" fontSize={epic ? 15 : 13}
              className="fill-secondary" style={{ fontWeight: epic || isSelected ? 600 : 400 }}>
              {name}
            </text>
          </g>
        );
      })}
    </svg>
  );
}
