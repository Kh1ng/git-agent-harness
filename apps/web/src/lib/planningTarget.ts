import type { PlanningMap, PlanningTarget } from '@git-agent-harness/contracts';

/** A `.plan/maps/` slug as chartr writes it; mirrors the server and CLI. */
export const validMapSlug = (slug: string) => /^[a-z0-9][a-z0-9-]{0,99}$/.test(slug);

/** The Planning page's selection as one string for a `<select>`:
 * `epic:900` for an issue epic, `file:node-handoff` for a map file. */
export type PlanningChoice = `epic:${number}` | `file:${string}`;

export function choiceTarget(choice: PlanningChoice): PlanningTarget {
  return choice.startsWith('epic:') ? { epic: Number(choice.slice(5)) } : { file: choice.slice(5) };
}

export function targetChoice(target: PlanningTarget): PlanningChoice {
  return 'epic' in target ? `epic:${target.epic}` : `file:${target.file}`;
}

/** `#12` for an issue; `02` for a map-file ticket, which is not an issue. */
export function nodeName(map: Pick<PlanningMap, 'file'>, number: number): string {
  return map.file ? String(number).padStart(2, '0') : `#${number}`;
}
