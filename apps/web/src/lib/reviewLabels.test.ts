import type { MergeRequest } from '@git-agent-harness/contracts';
import { ciLabelFor, stateLabelFor } from './reviewLabels.js';

const mr: MergeRequest = {
  branch: 'fix', id: '1', url: null, state: 'REVIEW_REQUIRED', draft: false,
  merge_status: null, merged: false, ci_passed: true, ci_pending: false,
  review_contract_version: 1, classification: 'ready', recommended_action: 'NONE',
};
test('labels CI with pending taking precedence over passed', () => {
  expect(ciLabelFor(mr)).toBe('Passed');
  expect(ciLabelFor({ ...mr, ci_pending: true })).toBe('Pending');
  expect(ciLabelFor({ ...mr, ci_passed: false })).toBe('Not passing');
});
test('missing CI evidence stays unknown', () => {
  expect(ciLabelFor()).toBe('Unknown');
  expect(ciLabelFor(null)).toBe('Unknown');
});
test('formats provider state and falls back to publication evidence', () => {
  expect(stateLabelFor(mr)).toBe('review required');
  expect(stateLabelFor({ ...mr, state: null })).toBe('not published');
  expect(stateLabelFor(null, true)).toBe('open');
  expect(stateLabelFor(undefined, false, true)).toBe('open');
  expect(stateLabelFor()).toBe('not published');
});
