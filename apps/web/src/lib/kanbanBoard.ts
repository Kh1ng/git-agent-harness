import type { ActiveClaim, AvailableTicket, Blocker, ControllerActivity, DeviceAgent, MergeRequest, QuotaSnapshot, StatusSnapshot } from '@git-agent-harness/contracts';
import { formatUntil, providerLabel, subscriptionUsage } from './subscriptionUsage.js';
import { workKey } from './workKey.js';

/**
 * The Kanban page's board, derived from records the server already serves:
 * the status snapshot, the quota snapshot, controller runs and the device's
 * agent processes. Nothing here is stored; every column, reason and verdict
 * is recomputed from those on each refresh.
 */

export const KANBAN_COLUMNS = [
  { key: 'ready', label: 'Ready', hint: 'Can start as soon as an agent picks it up' },
  { key: 'waiting', label: 'Waiting', hint: 'Cannot start yet' },
  { key: 'running', label: 'Running', hint: 'An agent is building it' },
  { key: 'review', label: 'In review', hint: 'Pull request open, waiting on or under review' },
  { key: 'fixing', label: 'Fixing', hint: 'Review or CI asked for changes' },
  { key: 'needs_you', label: 'Needs you', hint: 'Stopped until a person decides' },
  { key: 'done', label: 'Done', hint: 'Merged or closed' }
] as const;

export type KanbanColumnKey = (typeof KANBAN_COLUMNS)[number]['key'];
/** The next job a card needs an agent for. */
export type KanbanJob = 'build' | 'review' | 'fix' | 'merge';
export type KanbanTone = 'good' | 'warning' | 'critical' | 'unknown';

export interface KanbanSkip {
  backend: string;
  model: string | null;
  /** The router's own word for it: max_concurrent_reached, authentication_error… */
  reason: string;
}

export interface KanbanCard {
  key: string;
  workId: string | null;
  title: string;
  column: KanbanColumnKey;
  /** One line, plain words: why the card is where it is. */
  reason: string;
  tone: KanbanTone;
  /** An agent is on it right now. */
  working: boolean;
  /** Subscription and model: the one working on it, or the one that built its pull request. */
  agent: string | null;
  agentRole: 'working' | 'built' | null;
  /** Dispatch attempts so far. */
  attempts: number;
  /** Fix rounds used on its pull request, against the profile's limit. */
  fix: { used: number; max: number } | null;
  /** When the agent started (working), else the last time anything happened to it. */
  since: string | null;
  pullRequest: { id: string | null; url: string | null; draft: boolean } | null;
  held: boolean;
  job: KanbanJob | null;
  /** Everything card-specific that keeps it from running, for the why-not view. */
  blocks: string[];
  /** Who the router passed over the last time it tried this card, and why. */
  skipped: KanbanSkip[];
}

export interface KanbanAgent {
  id: string;
  backend: string;
  /** Subscription name and exact model: `Codex · gpt-6.1-sol`. */
  name: string;
  subscription: string;
  models: string[];
  /** Job kinds its routes allow (review, improve, pm…). */
  modes: string[];
  state: 'working' | 'idle' | 'unavailable';
  jobs: { cardKey: string | null; label: string; model: string | null; since: string | null }[];
  /** Why it is not working, or what it is working on. */
  reason: string;
  usedPercent: number | null;
  usageLabel: string | null;
  resetsIn: string | null;
}

export interface KanbanGate {
  label: string;
  /** null: the server did not say. */
  ok: boolean | null;
  detail: string;
}

export interface KanbanBoard {
  cards: KanbanCard[];
  agents: KanbanAgent[];
  gates: KanbanGate[];
  /** Issues the factory looked at and declined to take. */
  notPickedUp: { workId: string | null; title: string; reason: string }[];
}

export interface KanbanInput {
  status: StatusSnapshot | null;
  quota: Pick<QuotaSnapshot, 'candidates'> | null;
  controllerRuns: ControllerActivity[];
  factoryAgents: DeviceAgent[];
  /** null while unknown. */
  loopRunning: boolean | null;
  now: number;
}

const BACKEND_LABELS: Record<string, string> = { claude: 'Claude', codex: 'Codex', agy: 'Antigravity', gemini: 'Gemini', opencode: 'OpenCode', vibe: 'Vibe' };
const backendLabel = (backend: string) => BACKEND_LABELS[backend.toLowerCase()] ?? backend.charAt(0).toUpperCase() + backend.slice(1);
/** `Codex · gpt-6.1-sol`: the subscription and the exact model. */
export const agentLabel = (backend: string, model: string | null | undefined) => `${backendLabel(backend)} · ${model || 'model not reported'}`;

const humanize = (code: string) => code.replace(/[_-]+/g, ' ').trim();
const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

/** The router's skip reasons, short for a card and long for the why-not view. */
function plainSkip(reason: string): { short: string; long: string } {
  const lower = reason.toLowerCase();
  if (lower.includes('max_concurrent')) return { short: 'at its job limit', long: 'Already running as many jobs as it is allowed at once' };
  if (lower.includes('authentication')) return { short: 'sign-in failed', long: 'Sign-in failed; it needs to be logged in again' };
  if (lower.includes('already_attempted')) return { short: 'already tried and failed', long: 'Already tried this job and could not finish it' };
  if (/quota|rate.?limit|usage.?limit|exhaust/.test(lower)) return { short: 'allowance used up', long: 'Allowance used up' };
  if (lower.includes('cooldown')) return { short: 'cooling down', long: 'Cooling down after an error' };
  return { short: humanize(reason), long: sentence(humanize(reason)) };
}

/** `…; skipped: claude/opus[1m]: max_concurrent_reached, codex/gpt-6.1-sol: model-specific authentication_error route_state=…` */
export function parseSkipped(outcome: string | null | undefined): KanbanSkip[] {
  const list = /skipped: (.*?)(?:\s+route_state=\S+)?\s*$/s.exec(outcome ?? '')?.[1];
  if (!list) return [];
  return list.split(/,\s+(?=[^,/]+\/)/).map((entry) => {
    const cut = entry.lastIndexOf(': ');
    const route = cut < 0 ? entry : entry.slice(0, cut);
    const slash = route.indexOf('/');
    return {
      // `agy:google-native/…` names an instance of the agy backend.
      backend: (slash < 0 ? route : route.slice(0, slash)).split(':')[0].trim(),
      model: slash < 0 ? null : route.slice(slash + 1).trim(),
      reason: cut < 0 ? '' : entry.slice(cut + 2).trim()
    };
  });
}

const RUN_JOBS: Record<string, KanbanJob> = { dispatch_ticket: 'build', retry: 'build', review: 'review', fix_existing: 'fix', merge: 'merge' };
const runJob = (run: ControllerActivity): KanbanJob | null => RUN_JOBS[run.action.split(':')[0].trim()] ?? null;
/** `fix_existing: gah/…` names the branch the run works on. */
const runBranch = (run: ControllerActivity): string | null => {
  const target = run.action.slice(run.action.indexOf(':') + 1).trim();
  return target.includes('/') ? target : null;
};
/** The factory names a worktree after its branch, slashes flattened. */
const worktreeSlug = (branch: string) => branch.replace(/\//g, '-');
const cwdSlug = (cwd: string | null) => cwd?.replace(/\/+$/, '').split('/').pop() ?? null;

function plainFailure(failureClass: string | null): string {
  switch (failureClass) {
    case 'harness_error': return 'the harness failed before the agent could finish';
    case 'merge_failed': return 'the merge failed';
    case 'already_satisfied': return 'the agent found nothing left to change';
    case null: return 'no pull request came out of it';
    default: return humanize(failureClass);
  }
}

/** What a failed run says about why the card is not moving. */
function plainOutcome(run: ControllerActivity): string | null {
  const outcome = run.outcome ?? '';
  if (/refusing to replace existing checkpoint worktree/.test(outcome)) return 'Retry refused: saved work from an earlier attempt is still in the way';
  if (/not mergeable|cannot be cleanly created/.test(outcome)) return 'Merge refused: it conflicts with the main branch';
  if (/review requesting changes/.test(outcome)) return 'Merge refused: a GitHub review is still requesting changes';
  if (/made no repository progress/.test(outcome)) return 'The agent stalled before changing anything';
  return null;
}

function plainBlock(blocker: Blocker | undefined, ticket: AvailableTicket | null, fixMax: number): string {
  const code = blocker?.reason_code ?? blocker?.reason ?? ticket?.human_required_reason_code ?? null;
  switch (code) {
    case 'review_evidence_gate': return 'Approved by review, but it needs your sign-off before it can merge';
    case 'stuck_loop_gate': return 'Stuck: the same step repeated with no change, so it was stopped';
    case 'publishing_restriction': return 'Finished, but it may not publish a pull request here';
    case 'fix_retry_cap_exceeded': return `Parked: past the fix limit (${fixMax} fixes used)`;
    case null: return 'Needs a person to decide';
    default: return sentence(humanize(code));
  }
}

const latest = (...times: (string | null | undefined)[]) =>
  times.filter((time): time is string => !!time && Number.isFinite(Date.parse(time))).sort((a, b) => Date.parse(b) - Date.parse(a))[0] ?? null;

/** `Claude at its job limit, Codex sign-in failed` */
const skippedSummary = (skipped: KanbanSkip[]) => skipped.map((skip) => `${backendLabel(skip.backend)} ${plainSkip(skip.reason).short}`).join(', ');

function buildGates(input: KanbanInput, busyWorkers: number): KanbanGate[] {
  const { status, loopRunning } = input;
  const gates: KanbanGate[] = [{
    label: 'Loop',
    ok: loopRunning,
    detail: loopRunning === null ? 'Not reported' : loopRunning ? 'Running' : 'Stopped: nothing new starts until it runs'
  }];
  if (!status) return gates;
  gates.push({
    label: 'Worker slots',
    ok: busyWorkers < status.max_parallel_workers,
    detail: `${busyWorkers} of ${status.max_parallel_workers} busy`
  });
  const cap = status.profile.max_open_managed_mrs;
  gates.push({
    label: 'Open pull requests',
    ok: !status.implementation_intake_paused,
    detail: `${status.open_managed_mr_count} of ${cap} allowed${status.implementation_intake_paused ? ': new work is paused until some merge' : ''}`
  });
  for (const blocker of status.blockers) gates.push({ label: 'Profile blocked', ok: false, detail: blocker.message ?? blocker.reason ?? humanize(blocker.kind) });
  return gates;
}

function readyReason(gates: KanbanGate[], anyAgentFree: boolean): { reason: string; tone: KanbanTone } {
  const closed = gates.find((gate) => gate.ok === false);
  if (closed) return { reason: `Ready, but ${closed.label.toLowerCase()}: ${closed.detail.charAt(0).toLowerCase()}${closed.detail.slice(1)}`, tone: 'warning' };
  if (!anyAgentFree) return { reason: 'Ready, but no agent is free to take it', tone: 'warning' };
  return { reason: "Next in line: starts on the loop's next pass", tone: 'good' };
}

export function buildKanban(input: KanbanInput): KanbanBoard {
  const { status, controllerRuns, factoryAgents } = input;
  const runs = [...controllerRuns].sort((a, b) => Date.parse(b.started_at) - Date.parse(a.started_at));
  const tickets = status?.available_tickets ?? [];
  const mergeRequests = status?.merge_requests ?? [];
  const claims = status?.active_claims ?? [];
  const fixMax = status?.profile.max_fix_attempts_per_mr ?? 0;

  const ticketKey = (ticket: AvailableTicket) => workKey(ticket.work_id ?? ticket.normalized_work_identity ?? ticket.ticket_path);
  const mergeRequestKey = (mergeRequest: MergeRequest) => workKey(mergeRequest.work_id ?? mergeRequest.id ?? mergeRequest.branch);
  const keyOfBranch = new Map(mergeRequests.map((mergeRequest) => [mergeRequest.branch.toLowerCase(), mergeRequestKey(mergeRequest)]));
  /** A blocker or run names its work by issue, or only by branch. */
  const resolveKey = (reference: string) => keyOfBranch.get(reference.toLowerCase()) ?? workKey(reference);

  const runsByKey = new Map<string, ControllerActivity[]>();
  for (const run of runs) {
    const branch = runBranch(run);
    const key = run.work_id ? workKey(run.work_id) : branch ? resolveKey(branch) : null;
    if (key) runsByKey.set(key, [...(runsByKey.get(key) ?? []), run]);
  }
  const claimByKey = new Map<string, ActiveClaim>(claims.map((claim) => [workKey(claim.work_id), claim]));
  const blockerByKey = new Map<string, Blocker>();
  for (const blocker of status?.blocked_work_items ?? []) {
    const reference = blocker.remediation_plan?.work_id ?? blocker.source_reference;
    if (reference) blockerByKey.set(resolveKey(reference), blocker);
  }
  const dependencyByKey = new Map((status?.dependency_blockers ?? []).map((blocker) => [workKey(blocker.work_id), blocker]));
  const held = new Set((status?.review_held_work_ids ?? []).map(workKey));

  // Which card a factory agent's worktree belongs to.
  const keyOfSlug = new Map<string, string>();
  for (const mergeRequest of mergeRequests) keyOfSlug.set(worktreeSlug(mergeRequest.branch), mergeRequestKey(mergeRequest));
  for (const run of runs) {
    const branch = runBranch(run);
    if (branch && run.work_id && !keyOfSlug.has(worktreeSlug(branch))) keyOfSlug.set(worktreeSlug(branch), workKey(run.work_id));
  }
  const agentsOnCard = new Map<string, DeviceAgent[]>();
  for (const agent of factoryAgents) {
    const key = keyOfSlug.get(cwdSlug(agent.cwd) ?? '');
    if (key) agentsOnCard.set(key, [...(agentsOnCard.get(key) ?? []), agent]);
  }

  const busyWorkers = Math.max(claims.length, runs.filter((run) => run.status === 'running').length);
  const gates = buildGates(input, busyWorkers);
  const candidates = input.quota?.candidates ?? [];
  const anyAgentFree = candidates.length === 0 || candidates.some((candidate) => candidate.eligible_now);

  const items = new Map<string, { ticket: AvailableTicket | null; mergeRequest: MergeRequest | undefined }>();
  for (const ticket of tickets) items.set(ticketKey(ticket), { ticket, mergeRequest: undefined });
  for (const mergeRequest of mergeRequests) {
    const key = mergeRequestKey(mergeRequest);
    const existing = items.get(key);
    // An open pull request describes the work better than an older merged one.
    if (existing?.mergeRequest && !existing.mergeRequest.merged) continue;
    items.set(key, { ticket: existing?.ticket ?? null, mergeRequest });
  }

  const cards: KanbanCard[] = [];
  for (const [key, { ticket, mergeRequest }] of items) {
    const cardRuns = runsByKey.get(key) ?? [];
    const runningRun = cardRuns.find((run) => run.status === 'running');
    const lastRun = cardRuns.find((run) => run.status !== 'running');
    const claim = claimByKey.get(key);
    const blocker = blockerByKey.get(key);
    const fixUsed = mergeRequest ? status?.fix_attempt_counts[mergeRequest.branch] ?? 0 : 0;
    const processes = agentsOnCard.get(key) ?? [];
    const skipped = lastRun?.status === 'failed' ? parseSkipped(lastRun.outcome) : [];
    const lastOutcome = lastRun?.status === 'failed' ? plainOutcome(lastRun) : null;
    const working = !mergeRequest?.merged && (runningRun !== undefined || claim !== undefined);
    const blocks: string[] = [];

    let column: KanbanColumnKey;
    let reason: string;
    let tone: KanbanTone = 'unknown';
    let job: KanbanJob | null = null;

    if (mergeRequest?.merged) {
      column = 'done';
      reason = 'Merged';
      tone = 'good';
    } else if (working) {
      job = (runningRun && runJob(runningRun)) ?? (mergeRequest ? (mergeRequest.classification === 'NEEDS_REVIEW' ? 'review' : 'fix') : 'build');
      column = job === 'build' ? 'running' : job === 'fix' ? 'fixing' : 'review';
      reason = job === 'build' ? 'An agent is building it now' : job === 'fix' ? 'An agent is fixing what review or CI flagged' : job === 'merge' ? 'Merging now' : 'Under review now';
      tone = 'good';
    } else if (blocker || ticket?.human_required) {
      column = 'needs_you';
      reason = plainBlock(blocker, ticket, fixMax);
      tone = 'critical';
      blocks.push(blocker?.message && blocker.message !== 'Ledger indicates human intervention required' ? sentence(blocker.message) : reason);
      if (mergeRequest?.review_gate_reason) blocks.push(`Review gate: ${mergeRequest.review_gate_reason}`);
      for (const action of blocker?.remediation_plan?.safe_actions ?? []) blocks.push(`Next step: ${action.summary}`);
    } else if (mergeRequest) {
      const waitingOn = skipped.length ? `: ${skippedSummary(skipped)}` : '';
      switch (mergeRequest.classification) {
        case 'READY_FOR_HUMAN':
          column = 'needs_you';
          job = 'merge';
          reason = lastOutcome ?? 'Approved: waiting for your merge decision';
          tone = 'warning';
          blocks.push(reason);
          break;
        case 'NEEDS_FIX':
        case 'CI_FAILED':
          column = 'fixing';
          job = 'fix';
          reason = lastOutcome ?? `${mergeRequest.classification === 'CI_FAILED' ? 'CI failed' : 'Review asked for changes'}; waiting for a fixer${waitingOn}`;
          tone = 'warning';
          break;
        case 'CLOSED_UNMERGED':
          column = 'done';
          reason = 'Closed without merging';
          break;
        case 'STALE':
          column = 'waiting';
          reason = 'Its pull request went stale';
          tone = 'warning';
          break;
        default:
          column = 'review';
          job = 'review';
          reason = lastOutcome ?? `Waiting for a reviewer${waitingOn}`;
          tone = 'warning';
          if (mergeRequest.review_gate_reason) blocks.push(sentence(mergeRequest.review_gate_reason));
      }
      if (mergeRequest.ci_pending) blocks.push('CI is still running');
      if (fixMax > 0 && fixUsed >= fixMax && column === 'fixing') blocks.push(`It has used all ${fixMax} fix rounds; the next failure parks it`);
    } else {
      job = 'build';
      const dependency = dependencyByKey.get(key);
      const policy = ticket?.execution_policy;
      const attempts = ticket?.prior_attempt_count ?? 0;
      if (dependency) {
        const open = dependency.dependencies.filter((item) => item.normalized_state !== 'closed').map((item) => item.identity);
        column = 'waiting';
        reason = open.length ? `Blocked by ${open.join(', ')}` : sentence(dependency.reason);
        tone = 'warning';
      } else if (policy && !policy.dispatchable_now) {
        column = 'waiting';
        reason = sentence(policy.exclusion_reason ?? humanize(policy.exclusion_reason_code ?? 'not dispatchable now'));
        tone = 'warning';
      } else if (attempts > 0) {
        column = 'waiting';
        reason = lastOutcome ?? (skipped.length ? `No agent could take it: ${skippedSummary(skipped)}` : `Attempt ${attempts} ended without a pull request: ${plainFailure(ticket?.last_failure_class ?? null)}`);
        tone = 'warning';
      } else {
        column = 'ready';
        ({ reason, tone } = readyReason(gates, anyAgentFree));
      }
      if (column === 'waiting') blocks.push(reason);
    }
    if (held.has(key) && column !== 'done') blocks.push('A review hold is on: it will not merge automatically until the hold is released');

    const process = processes[0];
    const agent = working
      ? process ? agentLabel(process.tool, process.model) : null
      : mergeRequest?.effective_backend ? agentLabel(mergeRequest.effective_backend, mergeRequest.effective_model) : null;
    const evidence = status?.work_waypoint_evidence?.[ticket?.work_id ?? mergeRequest?.work_id ?? ''];
    cards.push({
      key,
      workId: ticket?.work_id ?? mergeRequest?.work_id ?? null,
      title: ticket?.title ?? mergeRequest?.title ?? ticket?.work_id ?? mergeRequest?.branch ?? key,
      column,
      reason,
      tone,
      working,
      agent,
      agentRole: agent ? (working ? 'working' : 'built') : null,
      attempts: ticket?.prior_attempt_count ?? 0,
      fix: mergeRequest && !mergeRequest.merged && fixMax > 0 ? { used: fixUsed, max: fixMax } : null,
      since: working
        ? latest(runningRun?.started_at, claim?.claimed_at)
        : mergeRequest?.merged ? mergeRequest.merged_at ?? null
          : latest(lastRun?.finished_at, lastRun?.started_at) ?? latest(evidence?.first_pull_request_at, evidence?.first_validation_at, evidence?.first_commit_at, evidence?.first_dispatch_at),
      pullRequest: mergeRequest ? { id: mergeRequest.id, url: mergeRequest.url, draft: mergeRequest.draft } : null,
      held: held.has(key) && column !== 'done',
      job,
      blocks,
      skipped
    });
  }
  // Work in hand first, then whatever moved most recently; Ready keeps the queue's order.
  const order = new Map(cards.map((card, index) => [card.key, index]));
  cards.sort((a, b) => Number(b.working) - Number(a.working)
    || (a.column === 'ready' && b.column === 'ready' ? order.get(a.key)! - order.get(b.key)! : (Date.parse(b.since ?? '') || 0) - (Date.parse(a.since ?? '') || 0)));

  return {
    cards,
    agents: buildAgents(input, cards, factoryAgents, keyOfSlug),
    gates,
    notPickedUp: (status?.issue_intake_rejections ?? []).map((rejection) => ({ workId: rejection.work_id, title: rejection.title ?? rejection.ticket_path, reason: sentence(rejection.reason) }))
  };
}

/** The job kinds an agent's routes must allow for it to take a card's next job. */
const JOB_MODES: Record<KanbanJob, string[]> = { build: ['improve', 'fix'], fix: ['improve', 'fix'], review: ['review'], merge: [] };
const JOB_NOUN: Record<KanbanJob, string> = { build: 'building', fix: 'fix', review: 'review', merge: 'merge' };
const canDo = (agent: Pick<KanbanAgent, 'modes'>, job: KanbanJob) => agent.modes.length === 0 || JOB_MODES[job].length === 0 || JOB_MODES[job].some((mode) => agent.modes.includes(mode));

function plainUnavailable(reason: string | null, until: string | null, now: number): string {
  const back = formatUntil(until, now);
  const why = reason ? plainSkip(reason).long : 'Not available right now';
  return back && back !== 'now' ? `${why}; back in ${back}` : why;
}

function buildAgents(input: KanbanInput, cards: KanbanCard[], factoryAgents: DeviceAgent[], keyOfSlug: Map<string, string>): KanbanAgent[] {
  const { quota, loopRunning, now, status } = input;
  const usage = new Map(subscriptionUsage(quota).map((subscription) => [subscription.id, subscription]));
  const cardByKey = new Map(cards.map((card) => [card.key, card]));
  const idleCards = cards.filter((card) => !card.working && card.job && card.job !== 'merge' && card.column !== 'needs_you' && card.column !== 'done');

  const agents = new Map<string, KanbanAgent & { eligible: boolean; unavailable: string | null }>();
  for (const candidate of quota?.candidates ?? []) {
    const id = candidate.backend_instance ?? candidate.backend;
    const existing = agents.get(id);
    const unavailable = candidate.eligible_now ? null : plainUnavailable(candidate.reason ?? null, candidate.unavailable_until ?? null, now);
    if (existing) {
      if (candidate.model && !existing.models.includes(candidate.model)) existing.models.push(candidate.model);
      for (const mode of candidate.modes) if (!existing.modes.includes(mode)) existing.modes.push(mode);
      existing.eligible ||= candidate.eligible_now;
      existing.unavailable ??= unavailable;
      continue;
    }
    const window = usage.get(id)?.tightest ?? null;
    agents.set(id, {
      id,
      backend: candidate.backend,
      name: '',
      subscription: `${providerLabel(candidate.provider, candidate.backend)} subscription`,
      models: candidate.model ? [candidate.model] : [],
      modes: [...candidate.modes],
      state: 'idle',
      jobs: [],
      reason: '',
      usedPercent: window?.usedPercent ?? null,
      usageLabel: window?.label ?? null,
      resetsIn: formatUntil(window?.resetAt ?? null, now),
      eligible: candidate.eligible_now,
      unavailable
    });
  }
  // A CLI the factory is running without a routing candidate still shows up.
  for (const process of factoryAgents) {
    if ([...agents.values()].some((agent) => agent.backend === process.tool)) continue;
    agents.set(process.tool, { id: process.tool, backend: process.tool, name: '', subscription: 'No quota data', models: [], modes: [], state: 'idle', jobs: [], reason: '', usedPercent: null, usageLabel: null, resetsIn: null, eligible: true, unavailable: null });
  }

  for (const agent of agents.values()) {
    for (const process of factoryAgents.filter((item) => item.tool === agent.backend)) {
      const card = cardByKey.get(keyOfSlug.get(cwdSlug(process.cwd) ?? '') ?? '');
      agent.jobs.push({
        cardKey: card?.key ?? null,
        label: card ? `${card.workId ?? card.title}${card.job ? ` (${JOB_NOUN[card.job]})` : ''}` : cwdSlug(process.cwd) ?? 'a factory job',
        model: process.model ?? null,
        since: process.started_at
      });
    }
    // The model on the running process is the exact one; a route only names what was configured.
    agent.name = agentLabel(agent.backend, agent.jobs.find((job) => job.model)?.model ?? agent.models.join(', '));
    const constraint = status?.constraints.find((item) => item.backend === agent.backend && item.reason);
    const takeable = idleCards.filter((card) => canDo(agent, card.job!));
    if (agent.jobs.length) {
      agent.state = 'working';
      agent.reason = `Working on ${agent.jobs.map((job) => job.label).join(', ')}`;
    } else if (!agent.eligible) {
      agent.state = 'unavailable';
      agent.reason = agent.unavailable ?? 'Not available right now';
    } else if (agent.usedPercent !== null && agent.usedPercent >= 99) {
      agent.state = 'unavailable';
      agent.reason = `Allowance used up${agent.resetsIn ? `; resets in ${agent.resetsIn}` : ''}`;
    } else if (loopRunning === false) {
      agent.reason = 'Idle: the loop is stopped';
    } else if (idleCards.length === 0) {
      agent.reason = 'Idle: no work is waiting';
    } else if (takeable.length === 0) {
      const kinds = [...new Set(idleCards.map((card) => JOB_NOUN[card.job!]))].join(' and ');
      agent.reason = `Idle: only ${kinds} jobs are waiting, and it is not set up for those`;
    } else {
      const passedOver = takeable.find((card) => card.skipped.some((item) => item.backend === agent.backend));
      const skip = passedOver?.skipped.find((item) => item.backend === agent.backend);
      agent.reason = passedOver && skip
        ? `Idle: passed over for ${passedOver.workId ?? passedOver.title} (${plainSkip(skip.reason).short})`
        : "Free: picks up work on the loop's next pass";
    }
    if (constraint && agent.state !== 'unavailable') agent.reason += `. ${constraint.model ?? 'One model'}: ${plainSkip(constraint.reason!).short}`;
  }
  const rank = { working: 0, idle: 1, unavailable: 2 };
  return [...agents.values()]
    .map(({ eligible: _eligible, unavailable: _unavailable, ...agent }) => agent)
    .sort((a, b) => rank[a.state] - rank[b.state] || a.name.localeCompare(b.name));
}

export interface KanbanVerdict {
  agent: KanbanAgent;
  /** Could take this card right now. */
  ok: boolean;
  verdict: string;
}

/** "Why isn't this running?": each agent with the rule that keeps it off this card. */
export function whyNotRunning(board: KanbanBoard, card: KanbanCard): KanbanVerdict[] {
  return board.agents.map((agent) => {
    const skip = card.skipped.find((item) => item.backend === agent.backend);
    if (card.job && !canDo(agent, card.job)) return { agent, ok: false, verdict: `Not set up for ${JOB_NOUN[card.job]} jobs` };
    if (skip) return { agent, ok: false, verdict: `${plainSkip(skip.reason).long}${skip.model ? ` (${skip.model})` : ''}` };
    if (agent.state === 'unavailable') return { agent, ok: false, verdict: agent.reason };
    if (agent.state === 'working') return { agent, ok: false, verdict: `Busy: ${agent.reason.charAt(0).toLowerCase()}${agent.reason.slice(1)}` };
    return { agent, ok: true, verdict: 'Available' };
  });
}
