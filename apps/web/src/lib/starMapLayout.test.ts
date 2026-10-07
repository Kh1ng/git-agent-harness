import type { PlanningMap, PlanningNode } from '@git-agent-harness/contracts';
import { layoutStarMap, STAR_MAP_SIZE } from './starMapLayout.js';
const node = (number: number, depth: number | null): PlanningNode => ({ number, depth, title: '', url: '', labels: [], state: 'ready', waiting_on: [] });
const map = (nodes: PlanningNode[], edges: PlanningMap['edges'] = []): PlanningMap => ({ epic: 1, nodes, edges, frontier: [], missing: [] });

test('centres the epic and places sorted children on a ring deterministically', () => {
  const input = map([node(1, 0), node(3, 1), node(2, 1)], [{ from: 1, to: 3, kind: 'child' }, { from: 1, to: 2, kind: 'child' }]);
  const positions = layoutStarMap(input);
  expect(positions.get(1)).toEqual({ x: STAR_MAP_SIZE / 2, y: STAR_MAP_SIZE / 2 });
  expect(positions.get(2)?.x).toBeCloseTo(672);
  expect(positions.get(2)?.y).toBeCloseTo(360);
  expect(positions.get(3)?.x).toBeCloseTo(48);
  expect(positions.get(3)?.y).toBeCloseTo(360);
  expect(layoutStarMap({ ...input, edges: [...input.edges].reverse() })).toEqual(positions);
});
test('handles an empty map and ignores invalid or duplicate child edges', () => {
  expect(layoutStarMap(map([]))).toEqual(new Map([[1, { x: 360, y: 360 }]]));
  const input = map([node(1, 0), node(2, 1)], [{ from: 1, to: 2, kind: 'child' }]);
  expect(layoutStarMap({ ...input, edges: [...input.edges, ...input.edges, { from: 99, to: 2, kind: 'child' }, { from: 2, to: 1, kind: 'child' }] })).toEqual(layoutStarMap(input));
});
test('places outside blockers separately beside their related child', () => {
  const positions = layoutStarMap(map([node(1, 0), node(2, 1), node(3, null), node(4, null)], [{ from: 1, to: 2, kind: 'child' }, { from: 3, to: 2, kind: 'blocks' }, { from: 4, to: 2, kind: 'blocks' }]));
  expect(positions.size).toBe(4);
  expect(positions.get(3)).not.toEqual(positions.get(4));
  for (const id of [3, 4]) {
    const position = positions.get(id)!;
    expect(Math.hypot(position.x - 360, position.y - 360)).toBeCloseTo(312);
  }
});
