import { useState, type ReactNode } from 'react';

/** Filter and bound a collection without hiding its current selection.
 * Search belongs to the caller so related collections can share one input.
 */
export function BoundedCollection<T>({ items, query, searchText, isSelected, label, emptyMessage, children }: {
  items: T[];
  query: string;
  searchText: (item: T) => string;
  isSelected: (item: T) => boolean;
  label: string;
  emptyMessage: string;
  children: (item: T) => ReactNode;
}) {
  const [expandedQuery, setExpandedQuery] = useState<string | null>(null);
  const normalized = query.trim().toLocaleLowerCase();
  const expanded = expandedQuery === normalized;
  const matches = items.filter((item) => searchText(item).toLocaleLowerCase().includes(normalized));
  const selectedIndex = items.findIndex(isSelected);
  const selected = items[selectedIndex];
  const selectionOutsideFilter = selectedIndex >= 0 && !matches.includes(selected);
  // Pin the selection before applying the bound so a long list cannot bury it.
  const candidates = selectedIndex >= 0
    ? [selected, ...matches.filter((item) => item !== selected)]
    : matches;
  const visible = expanded ? candidates : candidates.slice(0, 10);

  return (
    <>
      <p role="status" className="px-2 text-xs text-secondary">
        Matches: {matches.length} of {items.length}
        {candidates.length > visible.length ? ` · ${visible.length} rows shown` : ''}
        {selectionOutsideFilter ? ' · current selection also shown' : ''}
      </p>
      {matches.length === 0 && (
        <p className="px-2 py-2 text-sm text-secondary">{normalized ? `No ${label} match your search.` : emptyMessage}</p>
      )}
      {visible.map(children)}
      {candidates.length > 10 && (
        <button type="button" aria-expanded={expanded} onClick={() => setExpandedQuery(expanded ? null : normalized)}
          className="rounded px-2 py-2 text-sm text-secondary hover:bg-white/5 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
          {expanded ? 'Show fewer' : 'Show all'}
        </button>
      )}
    </>
  );
}
