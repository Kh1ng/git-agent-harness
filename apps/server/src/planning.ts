/**
 * Planning page API (#1241). The map is read-only: it comes from `gah map`,
 * which reads provider issues and never writes them. A planning chat is an
 * ordinary chat session whose opening message carries the map context and,
 * for grill-me, where the operator chose to record answers.
 */
import { Router } from 'express';
import rateLimit from 'express-rate-limit';
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, resolve } from 'node:path';
import {
  DEFAULT_PLANNING_SETTINGS,
  type ChatSessionSummary,
  type PlanningChatRequest,
  type PlanningEpicList,
  type PlanningMap,
  type PlanningNode,
  type PlanningSettings,
  type PlanningTarget
} from '@git-agent-harness/contracts';
import { AsyncTtlCache } from './asyncTtlCache.js';

export interface PlanningDeps {
  map(profile: string): Promise<PlanningEpicList>;
  map(profile: string, target: PlanningTarget): Promise<PlanningMap>;
  startChat(profile: string, seed: { title: string; text: string; worktree: boolean }, backend?: string): Promise<ChatSessionSummary>;
  settingsPath?: string;
}

function defaultSettingsPath(): string {
  return process.env.GAH_PLANNING_SETTINGS_PATH
    || resolve(process.env.XDG_CONFIG_HOME || resolve(homedir(), '.config'), 'gah/planning.json');
}

const validProfile = (value: unknown): value is string =>
  typeof value === 'string' && value.trim().length > 0 && value.length <= 128 && !/[\x00-\x1f\x7f]/.test(value);

const issueNumber = (value: unknown): number | null => {
  const text = typeof value === 'number' ? String(value) : value;
  return typeof text === 'string' && /^[1-9][0-9]{0,9}$/.test(text) ? Number(text) : null;
};

/** A `.plan/maps/` slug as chartr writes it; never a path. Mirrors the CLI. */
const mapSlug = (value: unknown): string | null =>
  typeof value === 'string' && /^[a-z0-9][a-z0-9-]{0,99}$/.test(value) ? value : null;

/** What a request maps: `epic=N` or `file=<slug>`, exactly one. */
function target(epic: unknown, file: unknown): PlanningTarget | null {
  if ((epic === undefined) === (file === undefined)) return null;
  if (epic !== undefined) {
    const number = issueNumber(epic);
    return number === null ? null : { epic: number };
  }
  const slug = mapSlug(file);
  return slug === null ? null : { file: slug };
}

const targetKey = (profile: string, chosen: PlanningTarget) =>
  'epic' in chosen ? `${profile}#${chosen.epic}` : `${profile}:file:${chosen.file}`;

/** A repository-relative Markdown path that cannot leave the checkout. */
export function validAnswersPath(path: unknown): path is string {
  return typeof path === 'string' && path.length > 0 && path.length <= 200 && path.endsWith('.md')
    && !path.startsWith('/') && !path.includes('\\') && !/[\x00-\x1f\x7f]/.test(path)
    && path.split('/').every((part) => part.length > 0 && part !== '.' && part !== '..');
}

export function readPlanningSettings(profile: string, path = defaultSettingsPath()): PlanningSettings {
  try {
    const stored = JSON.parse(readFileSync(path, 'utf8')) as Record<string, Partial<PlanningSettings>>;
    const entry = stored[profile];
    return {
      answers: entry?.answers === 'file' ? 'file' : 'issues',
      path: validAnswersPath(entry?.path) ? entry.path : DEFAULT_PLANNING_SETTINGS.path
    };
  } catch {
    return { ...DEFAULT_PLANNING_SETTINGS };
  }
}

function writePlanningSettings(profile: string, settings: PlanningSettings, path: string): void {
  let stored: Record<string, PlanningSettings> = {};
  if (existsSync(path)) {
    // Never overwrite a file we cannot read as an object: it may hold other profiles.
    const parsed: unknown = JSON.parse(readFileSync(path, 'utf8'));
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) throw new Error('planning settings are not an object');
    stored = parsed as Record<string, PlanningSettings>;
  }
  stored[profile] = settings;
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(stored, null, 2)}\n`, { mode: 0o600 });
  chmodSync(temporary, 0o600);
  renameSync(temporary, path);
}

/** `#12` for an issue; `ticket 02` for a map-file ticket, which is not one. */
function label(map: PlanningMap, number: number): string {
  return map.file ? `ticket ${String(number).padStart(2, '0')}` : `#${number}`;
}

function describeNode(map: PlanningMap, node: PlanningNode): string {
  const waiting = node.waiting_on.length > 0 ? `, waiting on ${node.waiting_on.map((n) => label(map, n)).join(' ')}` : '';
  const outside = node.depth === null ? ', outside the epic' : '';
  const where = node.path ? ` (${node.path})` : '';
  return `- ${label(map, node.number)} ${node.title}${where}: ${node.state}${waiting}${outside}`;
}

function mapSummary(map: PlanningMap): string[] {
  const epic = map.nodes.find((node) => node.number === map.epic);
  const rest = map.nodes.filter((node) => node.number !== map.epic);
  const heading = map.file
    ? `Planning map \`${map.file}\`${epic ? `: ${epic.title}` : ''} (.plan/maps/${map.file}/map.md, read-only here)`
    : `Epic #${map.epic}${epic ? ` ${epic.title} (${epic.url})` : ''}`;
  const frontier = map.frontier.map((n) => label(map, n)).join(' ');
  return [
    heading,
    '',
    'Current map:',
    ...(rest.length > 0 ? rest.map((node) => describeNode(map, node)) : [`- (no ${map.file ? 'tickets' : 'issues'} under it yet)`]),
    '',
    `Frontier (can start now): ${frontier || 'none'}`
  ];
}

/** The opening message for a grill-me session. */
export function grillPrompt(input: { map: PlanningMap | null; idea: string; settings: PlanningSettings }): string {
  const { map, idea, settings } = input;
  // A map file has no issue to parent new issues under.
  const parent = map && !map.file ? `\`Parent: #${map.epic}\` and ` : '';
  const record = settings.answers === 'file'
    ? [
      `   Write the decisions and tickets to \`${settings.path}\` in this session's worktree, under a heading with today's date, and commit it on this session's branch.`,
      '   Do not file issues.'
    ]
    : [
      '   File one issue per decision, labelled `wayfinder:grilling`, with Question and Answer sections.',
      '   File one issue per ticket, labelled `wayfinder:task`, with its acceptance criteria.',
      `   Put ${parent}\`Blocked by: #A, #B\` lines in each body so the planning map links them.`,
      '   Use `gh` for GitHub or `glab` for GitLab. List what you will file and wait for my yes before filing anything.'
    ];
  return [
    'Planning session (grill-me).',
    '',
    ...(map ? [...mapSummary(map), ''] : []),
    idea.trim() ? `The idea to plan: ${idea.trim()}` : 'Start by asking me what I want to plan.',
    '',
    'How to run this session:',
    '1. Ask me one question at a time. Cover what done looks like, what is in and out of scope, constraints, risks, and which parts depend on which. Push back on vague answers.',
    '2. Do not write code or change files while questioning.',
    '3. When the questions are answered, summarize the decisions and propose tickets, each with a title, acceptance criteria, and what blocks it.',
    '4. Record them only after I confirm the summary:',
    ...record
  ].join('\n');
}

/** The opening message for a discussion of one ticket on the map. */
export function ticketPrompt(map: PlanningMap, ticket: number): string {
  const node = map.nodes.find((candidate) => candidate.number === ticket);
  const blockers = map.edges
    .filter((edge) => edge.kind === 'blocks' && edge.to === ticket)
    .map((edge) => map.nodes.find((candidate) => candidate.number === edge.from))
    .filter((candidate): candidate is PlanningNode => candidate !== undefined);
  const where = map.file ? node?.path ?? '' : node?.url ?? '';
  const read = map.file
    ? `Read the ticket file and its blockers' files (their Answer sections hold the decisions it builds on).`
    : `Read the ticket and its blockers' discussions (\`gh issue view ${ticket} --comments\`, or \`glab issue view ${ticket} --comments\` on GitLab).`;
  return [
    `Ticket ${map.file ? String(ticket).padStart(2, '0') : `#${ticket}`}${node ? ` ${node.title}${where ? ` (${where})` : ''}` : ''}, from the planning map.`,
    '',
    ...(blockers.length > 0
      ? ['Its blockers, whose outcomes it builds on:', ...blockers.map((blocker) => describeNode(map, blocker)), '']
      : []),
    ...mapSummary(map),
    '',
    read,
    `Then help me understand it and plan the approach. This chat is for discussion: do not change files or the ${map.file ? 'map' : 'issue'}.`
  ].join('\n');
}

/** Auth is applied at mount. */
export function planningRouter(deps: PlanningDeps): Router {
  const settingsPath = deps.settingsPath ?? defaultSettingsPath();
  const maps = new AsyncTtlCache<string, PlanningEpicList | PlanningMap>(60_000);
  const loadMap = (profile: string, chosen: PlanningTarget): Promise<PlanningMap> =>
    maps.get(targetKey(profile, chosen), () => deps.map(profile, chosen)) as Promise<PlanningMap>;
  const router = Router();
  router.use((_req, res, next) => { res.setHeader('Cache-Control', 'no-store'); next(); });
  router.use(rateLimit({ windowMs: 60_000, limit: 60, standardHeaders: true, legacyHeaders: false,
    message: { error: 'rate_limited', message: 'Too many planning requests. Retry in a minute.' } }));

  const unavailable = (res: import('express').Response) => res.status(502).json({
    error: 'map_unavailable', message: 'Cannot read this map. Check `gah map` on the central node.'
  });

  router.get('/epics', async (req, res) => {
    if (!validProfile(req.query.profile)) { res.status(400).json({ error: 'invalid_request', message: 'Name a configured profile.' }); return; }
    const profile = req.query.profile;
    if (req.query.refresh === '1') maps.delete(profile);
    try { res.json(await maps.get(profile, () => deps.map(profile))); } catch { unavailable(res); }
  });

  router.get('/map', async (req, res) => {
    const chosen = target(req.query.epic, req.query.file);
    if (!validProfile(req.query.profile) || chosen === null) {
      res.status(400).json({ error: 'invalid_request', message: 'Name a configured profile and either an epic issue number or a .plan/maps/ map.' }); return;
    }
    if (req.query.refresh === '1') maps.delete(targetKey(req.query.profile, chosen));
    try { res.json(await loadMap(req.query.profile, chosen)); } catch { unavailable(res); }
  });

  router.get('/settings', (req, res) => {
    if (!validProfile(req.query.profile)) { res.status(400).json({ error: 'invalid_request', message: 'Name a configured profile.' }); return; }
    res.json(readPlanningSettings(req.query.profile, settingsPath));
  });

  router.put('/settings', (req, res) => {
    const body = req.body as Record<string, unknown> | undefined;
    if (!body || !validProfile(body.profile) || (body.answers !== 'issues' && body.answers !== 'file')
      || !validAnswersPath(body.path)) {
      res.status(400).json({ error: 'invalid_request', message: 'Answers go to `issues` or a repository-relative `.md` file.' }); return;
    }
    const settings: PlanningSettings = { answers: body.answers, path: body.path };
    try {
      writePlanningSettings(body.profile, settings, settingsPath);
    } catch {
      res.status(500).json({ error: 'settings_unavailable', message: 'Cannot save planning settings. Check the settings file on the central node.' }); return;
    }
    res.json(settings);
  });

  router.post('/chats', async (req, res) => {
    const body = req.body as Partial<PlanningChatRequest> | undefined;
    const scoped = body?.epic !== undefined || body?.file !== undefined;
    const chosen = scoped ? target(body?.epic, body?.file) : null;
    // A file map numbers its tickets from 1, and 0 is its destination.
    const ticket = body?.ticket === undefined ? null : issueNumber(body.ticket);
    const idea = typeof body?.idea === 'string' ? body.idea : '';
    const valid = body && validProfile(body.profile)
      && (body.backend === undefined || (typeof body.backend === 'string' && /^[a-z0-9_-]{1,64}$/.test(body.backend)))
      && idea.length <= 4000
      && (!scoped || chosen !== null)
      && ((body.kind === 'grill') || (body.kind === 'ticket' && chosen !== null && ticket !== null));
    if (!valid) {
      res.status(400).json({ error: 'invalid_request', message: 'A planning chat is `grill` (optional epic or map, and idea) or `ticket` (epic or map, and ticket).' }); return;
    }
    const profile = body.profile as string;
    let map: PlanningMap | null = null;
    if (chosen !== null) {
      try { map = await loadMap(profile, chosen); } catch { unavailable(res); return; }
    }
    const settings = readPlanningSettings(profile, settingsPath);
    const scope = chosen === null ? '' : 'epic' in chosen ? `: #${chosen.epic}` : `: ${chosen.file}`;
    const ticketName = map?.file ? String(ticket).padStart(2, '0') : `#${ticket}`;
    const seed = body.kind === 'grill'
      ? { title: `Plan${scope}`, text: grillPrompt({ map, idea, settings }), worktree: settings.answers === 'file' }
      : { title: `Ticket ${ticketName}`, text: ticketPrompt(map as PlanningMap, ticket as number), worktree: false };
    try {
      res.status(201).json(await deps.startChat(profile, seed, body.backend));
    } catch {
      res.status(422).json({ error: 'chat_unavailable', message: 'Cannot start the planning chat.' });
    }
  });
  return router;
}
