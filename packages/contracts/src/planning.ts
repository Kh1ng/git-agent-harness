/** `gah map --json` output and the Planning page API (#1241). The map is
 * read-only: it reflects provider issues, or a chartr `.plan/maps/` map in
 * the checkout, and never writes to either. */

/** `ruled_out` and `claimed` occur only in map files: a ruled-out ticket
 * does not unblock its dependents, and a claimed one is open but taken. */
export type PlanningNodeState = 'done' | 'ready' | 'blocked' | 'parent' | 'ruled_out' | 'claimed';

export interface PlanningNode {
  number: number;
  title: string;
  url: string;
  labels: string[];
  state: PlanningNodeState;
  /** Distance from the epic along child links; null for a blocker outside it. */
  depth: number | null;
  /** Open issues this one waits on. */
  waiting_on: number[];
  /** A map-file ticket's path in the repository (`url` is then empty). */
  path?: string;
}

export interface PlanningEdge {
  from: number;
  to: number;
  /** `child`: from is the parent of to. `blocks`: from must close before to. */
  kind: 'child' | 'blocks';
}

export interface PlanningMap {
  epic: number;
  nodes: PlanningNode[];
  edges: PlanningEdge[];
  /** Ready issues under the epic: the work that can start now. */
  frontier: number[];
  /** Referenced issues the provider listing did not include. */
  missing: number[];
  /** The `.plan/maps/` slug when the map came from files. Node 0 is then the
   * map's destination and the numbers are ticket numbers, not issues. */
  file?: string;
  /** Map-file problems that did not stop the map (a skipped ticket, say). */
  diagnostics?: string[];
}

export interface PlanningEpic {
  number: number;
  title: string;
  url: string;
  open: boolean;
  children: number;
  open_children: number;
}

/** A chartr map in the checkout's `.plan/maps/<slug>/`. */
export interface PlanningMapFile {
  slug: string;
  title: string;
  tickets: number;
  /** Tickets neither resolved nor ruled out. */
  open_tickets: number;
}

export interface PlanningEpicList {
  epics: PlanningEpic[];
  files: PlanningMapFile[];
  /** Why issues could not be listed; the map files still were. */
  issues_error?: string;
}

/** What to map: an epic issue, or a `.plan/maps/` slug. */
export type PlanningTarget = { epic: number } | { file: string };

/** Where a grill-me chat records answers: provider issues, or one Markdown
 * file in the repository at `path`. */
export interface PlanningSettings {
  answers: 'issues' | 'file';
  /** Repository-relative Markdown path, used when `answers` is `file`. */
  path: string;
}

export const DEFAULT_PLANNING_SETTINGS: PlanningSettings = {
  answers: 'issues',
  path: 'docs/PLANNING.md'
};

/** `grill`: question an idea (optionally within an epic) into decisions and
 * tickets. `ticket`: discuss one frontier ticket with its map context. */
export interface PlanningChatRequest {
  profile: string;
  kind: 'grill' | 'ticket';
  epic?: number;
  /** A `.plan/maps/` slug, instead of `epic`. */
  file?: string;
  ticket?: number;
  /** The rough idea to grill, for `grill`. */
  idea?: string;
  backend?: string;
}
