import type { PlanningMap } from '@git-agent-harness/contracts';

export interface StarPosition { x: number; y: number }

/** Coordinates in a SIZE x SIZE view box centred on the epic. */
export const STAR_MAP_SIZE = 720;
const CENTRE = STAR_MAP_SIZE / 2;
const OUTER_RADIUS = STAR_MAP_SIZE / 2 - 48;

/**
 * Radial layout: the epic at the centre, each level of children on its own
 * ring, and every subtree given an arc proportional to its leaf count so
 * siblings stay together. Blockers from outside the epic sit on an outer
 * ring next to the issues they block.
 */
export function layoutStarMap(map: PlanningMap): Map<number, StarPosition> {
  const depth = new Map(map.nodes.map((node) => [node.number, node.depth]));
  const children = new Map<number, number[]>();
  const placedChild = new Set<number>();
  for (const edge of map.edges) {
    if (edge.kind !== 'child' || placedChild.has(edge.to)) continue;
    const parentDepth = depth.get(edge.from);
    const childDepth = depth.get(edge.to);
    if (parentDepth == null || childDepth == null || childDepth !== parentDepth + 1) continue;
    placedChild.add(edge.to);
    children.set(edge.from, [...(children.get(edge.from) ?? []), edge.to]);
  }
  for (const list of children.values()) list.sort((a, b) => a - b);

  const leaves = new Map<number, number>();
  const countLeaves = (number: number): number => {
    const kids = children.get(number) ?? [];
    const total = kids.length === 0 ? 1 : kids.reduce((sum, kid) => sum + countLeaves(kid), 0);
    leaves.set(number, total);
    return total;
  };
  countLeaves(map.epic);

  const maxDepth = Math.max(0, ...map.nodes.map((node) => node.depth ?? 0));
  const hasOutside = map.nodes.some((node) => node.depth === null);
  const rings = maxDepth + (hasOutside ? 1 : 0);
  const ring = rings === 0 ? 0 : OUTER_RADIUS / rings;
  const positions = new Map<number, StarPosition>();
  const angles = new Map<number, number>();
  const at = (radius: number, angle: number): StarPosition => ({
    x: CENTRE + radius * Math.cos(angle),
    y: CENTRE + radius * Math.sin(angle)
  });

  // Start at twelve o'clock and go clockwise.
  const place = (number: number, level: number, from: number, to: number) => {
    const angle = (from + to) / 2;
    angles.set(number, angle);
    positions.set(number, level === 0 ? { x: CENTRE, y: CENTRE } : at(level * ring, angle));
    let cursor = from;
    for (const kid of children.get(number) ?? []) {
      const span = ((to - from) * (leaves.get(kid) ?? 1)) / (leaves.get(number) ?? 1);
      place(kid, level + 1, cursor, cursor + span);
      cursor += span;
    }
  };
  place(map.epic, 0, -Math.PI / 2, (3 * Math.PI) / 2);

  // Issues in the tree that no child edge reached (a second parent, say)
  // and outside blockers: beside what they relate to, on their own ring.
  const loose = map.nodes.filter((node) => !positions.has(node.number));
  const taken: number[] = [];
  for (const node of loose) {
    const related = map.edges
      .filter((edge) => edge.from === node.number || edge.to === node.number)
      .map((edge) => angles.get(edge.from === node.number ? edge.to : edge.from))
      .filter((angle): angle is number => angle !== undefined);
    let angle = related.length > 0
      ? Math.atan2(
        related.reduce((sum, value) => sum + Math.sin(value), 0),
        related.reduce((sum, value) => sum + Math.cos(value), 0)
      )
      : -Math.PI / 2 + taken.length * 0.5;
    // Nudge apart stars that would land on top of each other.
    while (taken.some((other) => Math.abs(Math.atan2(Math.sin(angle - other), Math.cos(angle - other))) < 0.12)) angle += 0.14;
    taken.push(angle);
    angles.set(node.number, angle);
    const level = node.depth ?? maxDepth + 1;
    positions.set(node.number, at(Math.max(1, level) * ring, angle));
  }
  return positions;
}
