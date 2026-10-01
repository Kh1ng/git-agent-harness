/** `gah map --json` output and the Planning page API (#1241). The map is
 * read-only: it reflects provider issues and never writes to them. */

export type PlanningNodeState = 'done' | 'ready' | 'blocked' | 'parent';

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
}

export interface PlanningEpic {
  number: number;
  title: string;
  url: string;
  open: boolean;
  children: number;
  open_children: number;
}

export interface PlanningEpicList {
  epics: PlanningEpic[];
}

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
  ticket?: number;
  /** The rough idea to grill, for `grill`. */
  idea?: string;
  backend?: string;
}
