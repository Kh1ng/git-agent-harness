/** Canonical work-identity normalization shared by every work surface
 * (Work page, Overview, blocked items, the detail drawer). Mutations and
 * evidence lookups key on this normalization, so the rule lives in one place. */
export function workKey(workId: string): string {
  const trimmed = workId.trim();
  if (/^\d+$/.test(trimmed)) return `#${Number(trimmed)}`;
  const issue = trimmed.match(/^#0*(\d+)$/);
  if (issue) return `#${Number(issue[1])}`;
  const ticket = trimmed.match(/^ticket-0*(\d+)$/i);
  if (ticket) return `#${Number(ticket[1])}`;
  return trimmed.toLowerCase();
}
