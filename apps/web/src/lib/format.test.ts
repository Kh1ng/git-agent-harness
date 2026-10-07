import type { Session } from '@git-agent-harness/contracts';
import * as format from './format.js';
const now = new Date('2026-01-02T12:00:00Z');

test('formats known numeric observations including real zeroes', () => {
  expect(format.formatCost(0)).toBe('$0.0000');
  expect(format.formatCost(0.125)).toBe('$0.1250');
  expect(format.formatCost(12.5)).toBe('$12.50');
  expect(format.formatTokens(0)).toBe('0');
  expect(format.formatTokens(1500)).toBe('1.5k');
  expect(format.formatTokens(2500000)).toBe('2.5M');
  expect(format.formatPercent(0.125, 1)).toBe('12.5%');
  expect(format.formatDuration(12.4)).toBe('12s');
  expect(format.formatDuration(120)).toBe('2m');
  expect(format.formatDuration(5400)).toBe('1.5h');
  expect(format.formatCount(0)).toBe('0');
  expect(format.formatCount(1234)).toBe((1234).toLocaleString());
});
test.each([null, undefined])('keeps missing numeric observations unknown (%s)', (missing) => {
  for (const formatter of [format.formatCost, format.formatTokens, format.formatPercent, format.formatDuration, format.formatCount]) {
    expect(formatter(missing)).toBe('Unknown');
  }
});
test('keeps NaN percent unknown', () => expect(format.formatPercent(NaN)).toBe('Unknown'));

test('formats remaining time using an explicit clock', () => {
  expect(format.formatRemaining('2026-01-02T15:24:00Z', now)).toBe('3h 24m');
  expect(format.formatRemaining('2026-01-02T12:08:00Z', now)).toBe('8m');
  expect(format.formatRemaining('2026-01-02T12:00:30Z', now)).toBe('<1m');
  expect(format.formatRemaining(now.toISOString(), now)).toBeNull();
  expect(format.formatRemaining('2026-01-01', now)).toBeNull();
});
test('formats observation age and future observations', () => {
  expect(format.formatAge('2026-01-02T11:52:00Z', now)).toBe('8m ago');
  expect(format.formatAge('2026-01-02T09:00:00Z', now)).toBe('3h ago');
  expect(format.formatAge('2025-12-31T12:00:00Z', now)).toBe('2d ago');
  expect(format.formatAge('2026-01-03', now)).toBe('just now');
});
test.each([null, undefined, '', 'malformed'])('handles unavailable timestamps %s', (input) => {
  expect(format.formatRemaining(input, now)).toBeNull();
  expect(format.formatAge(input, now)).toBeNull();
  expect(format.formatLocalTime(input)).toBeNull();
  expect(format.isStale(input, now)).toBe(false);
});
test('formats local time using the runtime locale', () => {
  expect(format.formatLocalTime(now.toISOString())).toBe(now.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }));
});
test('reports fetched age and the oldest available observation', () => {
  const clock = now.getTime();
  expect(format.formatUpdatedAge(null, clock)).toBe('never');
  expect(format.formatUpdatedAge(undefined, clock)).toBe('never');
  expect(format.formatUpdatedAge(clock + 1, clock)).toBe('just now');
  expect(format.formatUpdatedAge(clock - 3000, clock)).toBe('3s ago');
  expect(format.formatUpdatedAge(clock - 240000, clock)).toBe('4m ago');
  expect(format.formatUpdatedAge(clock - 7200000, clock)).toBe('2h ago');
  expect(format.formatUpdatedAge(clock - 172800000, clock)).toBe('2d ago');
  expect(format.oldestFetchedAt(null, 0, 5, undefined)).toBe(0);
  expect(format.oldestFetchedAt(null, undefined)).toBeNull();
  expect(format.oldestFetchedAt()).toBeNull();
});
test('marks observations stale only beyond the threshold', () => {
  expect(format.isStale(new Date(now.getTime() - format.STALE_THRESHOLD_MS).toISOString(), now)).toBe(false);
  expect(format.isStale(new Date(now.getTime() - format.STALE_THRESHOLD_MS - 1).toISOString(), now)).toBe(true);
});
test('formats dispatch and chat names with sensible fallbacks', () => {
  const session: Pick<Session, 'id' | 'mode' | 'mr' | 'target' | 'branch' | 'repo'> = { id: 's1', mode: 'review' };
  expect(format.formatDispatchName({ ...session, mr: '#42' })).toBe('Review PR #42');
  expect(format.formatDispatchName({ ...session, target: 'PR #42' })).toBe('Review PR #42');
  expect(format.formatDispatchName({ ...session, target: '42' })).toBe('Review PR #42');
  expect(format.formatDispatchName({ ...session, mode: 'pm', target: '猫' })).toBe('PM 猫');
  expect(format.formatDispatchName({ ...session, branch: 'fix' })).toBe('fix');
  expect(format.formatDispatchName({ ...session, repo: 'repo' })).toBe('repo');
  expect(format.formatDispatchName(session)).toBe('s1');
  expect(format.formatChatName({ id: 'c1', title: ' 猫 ' })).toBe('猫');
  expect(format.formatChatName({ id: 'c1', title: ' ' })).toBe('Chat c1');
  expect(format.formatChatName({ id: 'c1', title: null })).toBe('Chat c1');
});
