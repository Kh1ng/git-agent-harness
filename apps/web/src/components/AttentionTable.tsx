import { useMemo, useState } from 'react';
import { ArrowDown, ArrowUp, ArrowUpDown, ChevronLeft, ChevronRight, ShieldAlert } from 'lucide-react';
import type { Blocker, DependencyBlocker } from '@git-agent-harness/contracts';
import { StatusBadge } from './ui/StatusBadge.js';
import { BlockedWorkItem } from './BlockedWorkItems.js';

type Kind = 'profile' | 'dependency' | 'work' | 'review';
const KIND: Record<Kind, { label: string; tone: 'critical' | 'warning'; order: number }> = {
  profile: { label: 'Blocks all work', tone: 'critical', order: 0 },
  work: { label: 'Blocked', tone: 'warning', order: 1 },
  dependency: { label: 'Dependency', tone: 'warning', order: 2 },
  review: { label: 'Review hold', tone: 'warning', order: 3 }
};

export interface AttentionRow {
  key: string;
  kind: Kind;
  /** The work id, or the blocker kind for a profile-wide blocker. */
  work: string;
  summary: string;
  /** A blocked work item opens to its remediation plan. */
  blocker?: Blocker;
}

export function attentionRows(input: {
  blockers: Blocker[];
  dependencyBlockers: DependencyBlocker[];
  blockedWorkItems: Blocker[];
  reviewHeldWorkIds: string[];
}): AttentionRow[] {
  const rows: AttentionRow[] = [];
  input.blockers.forEach((b, i) => rows.push({ key: `profile-${i}`, kind: 'profile', work: b.kind.replace(/_/g, ' '), summary: b.message || b.reason || 'Unknown' }));
  input.blockedWorkItems.forEach((b, i) => rows.push({
    key: `work-${i}`, kind: 'work', work: b.source_reference ?? 'unknown',
    summary: [b.reason_code && b.reason_code !== 'unknown' ? b.reason_code.replace(/_/g, ' ') : 'unknown reason', b.message].filter(Boolean).join(': '),
    blocker: b
  }));
  input.dependencyBlockers.forEach((dep, i) => {
    const open = dep.dependencies.filter((d) => d.normalized_state !== 'closed');
    const blockedOn = (open.length > 0 ? open : dep.dependencies).map((d) => `${d.identity} [${d.normalized_state}]`).join(', ');
    rows.push({ key: `dependency-${i}`, kind: 'dependency', work: dep.work_id,
      summary: [dep.title, dep.reason, blockedOn ? `blocked on ${blockedOn}` : null].filter(Boolean).join(' · ') });
  });
  input.reviewHeldWorkIds.forEach((workId) => rows.push({ key: `review-${workId}`, kind: 'review', work: workId, summary: 'Manager review hold active' }));
  return rows;
}

type SortKey = 'kind' | 'work' | 'summary';
const PAGE_SIZE = 10;

/** Natural order for work ids: #946 before #1371. */
function compareWork(a: string, b: string): number {
  return a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' });
}

/**
 * Everything on the profile that waits for a person, as one sortable table
 * ten rows at a time: profile-wide blockers, blocked work items (each
 * opens to its remediation plan), dependency blockers and review holds.
 */
export function AttentionTable({ rows, onOpenWork }: { rows: AttentionRow[]; onOpenWork?: (workId: string) => void }) {
  const [sort, setSort] = useState<{ key: SortKey; dir: 1 | -1 }>({ key: 'kind', dir: 1 });
  const [page, setPage] = useState(0);
  const [open, setOpen] = useState<string | null>(null);

  const sorted = useMemo(() => [...rows].sort((a, b) => {
    const value = sort.key === 'kind' ? KIND[a.kind].order - KIND[b.kind].order || compareWork(a.work, b.work)
      : sort.key === 'work' ? compareWork(a.work, b.work)
      : a.summary.localeCompare(b.summary);
    return value * sort.dir;
  }), [rows, sort]);
  const pages = Math.max(1, Math.ceil(sorted.length / PAGE_SIZE));
  const current = Math.min(page, pages - 1);
  const visible = sorted.slice(current * PAGE_SIZE, current * PAGE_SIZE + PAGE_SIZE);

  const toggleSort = (key: SortKey) => {
    setSort((state) => ({ key, dir: state.key === key ? (state.dir === 1 ? -1 : 1) : 1 }));
    setPage(0);
  };
  const header = (key: SortKey, label: string) => {
    const active = sort.key === key;
    const Icon = !active ? ArrowUpDown : sort.dir === 1 ? ArrowUp : ArrowDown;
    return (
      <th scope="col" aria-sort={active ? (sort.dir === 1 ? 'ascending' : 'descending') : 'none'}>
        <button type="button" onClick={() => toggleSort(key)} className="inline-flex items-center gap-1 text-xs font-medium text-secondary hover:text-primary">
          {label}<Icon size={12} aria-hidden="true" />
        </button>
      </th>
    );
  };

  return (
    <section className="card overflow-hidden border-warning/30" aria-labelledby="needs-attention-title">
      <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
        <h3 id="needs-attention-title" className="flex items-center gap-2 text-sm font-semibold text-primary">
          <ShieldAlert size={16} className="text-warning" aria-hidden="true" />
          Needs attention
          <span className="text-xs font-normal tabular-nums text-muted">{rows.length}</span>
        </h3>
        {pages > 1 && (
          <nav aria-label="Needs attention pages" className="flex items-center gap-2 text-xs tabular-nums text-muted">
            <button type="button" onClick={() => setPage(current - 1)} disabled={current === 0} className="btn-secondary !min-h-0 !px-1.5 !py-1 disabled:opacity-40" aria-label="Previous page"><ChevronLeft size={14} aria-hidden="true" /></button>
            <span>{current * PAGE_SIZE + 1}–{Math.min(sorted.length, (current + 1) * PAGE_SIZE)} of {sorted.length}</span>
            <button type="button" onClick={() => setPage(current + 1)} disabled={current >= pages - 1} className="btn-secondary !min-h-0 !px-1.5 !py-1 disabled:opacity-40" aria-label="Next page"><ChevronRight size={14} aria-hidden="true" /></button>
          </nav>
        )}
      </div>
      <table className="table-base">
        <thead>
          <tr>
            {header('kind', 'Kind')}
            {header('work', 'Work')}
            {header('summary', 'Summary')}
          </tr>
        </thead>
        <tbody>
          {visible.map((row) => {
            const expandable = !!row.blocker;
            const expanded = open === row.key;
            return [
              <tr key={row.key} className={expandable ? 'cursor-pointer hover:bg-raised/50' : undefined}
                onClick={expandable ? () => setOpen(expanded ? null : row.key) : undefined}
                aria-expanded={expandable ? expanded : undefined}>
                <td className="whitespace-nowrap"><StatusBadge tone={KIND[row.kind].tone} label={KIND[row.kind].label} /></td>
                <td className="whitespace-nowrap font-mono text-xs">
                  {onOpenWork && row.kind !== 'profile' && !row.work.startsWith('http')
                    ? <button type="button" onClick={(event) => { event.stopPropagation(); onOpenWork(row.work); }} className="text-accent hover:underline">{row.work}</button>
                    : row.work}
                </td>
                <td className="max-w-0 text-xs text-secondary">
                  <span className={`block ${expanded ? 'whitespace-normal break-words' : 'truncate'}`} title={row.summary}>{row.summary}</span>
                  {expandable && !expanded && <span className="text-[11px] text-muted">Click for the remediation plan</span>}
                </td>
              </tr>,
              expandable && expanded && (
                <tr key={`${row.key}-plan`}>
                  <td colSpan={3} className="bg-raised/40">
                    <ul><BlockedWorkItem blocker={row.blocker!} onOpenWork={onOpenWork} /></ul>
                  </td>
                </tr>
              )
            ];
          })}
        </tbody>
      </table>
    </section>
  );
}
