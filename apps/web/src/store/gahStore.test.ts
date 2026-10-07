import type { ReportData } from '@git-agent-harness/contracts';
import { gahApi } from '../api/client.js';
import { useGahStore } from './gahStore.js';

jest.mock('../api/client.js', () => ({
  gahApi: { getReport: jest.fn() },
  GahApiError: class GahApiError extends Error {},
}));

const getReport = jest.mocked(gahApi.getReport);
const initialState = useGahStore.getState();
const report = (profile: string, total_entries = 1): ReportData => ({
  ledger_path: '/ledger', total_entries, since: '', profile, group_by: 'backend', comparisons: [], trend: [],
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  useGahStore.setState(initialState, true);
  getReport.mockReset();
  jest.useFakeTimers().setSystemTime(new Date('2026-01-01T00:00:00Z'));
});
afterEach(() => jest.useRealTimers());

test('two readers share the fetched report with one API call', async () => {
  const data = report('alpha');
  getReport.mockResolvedValue(data);
  await useGahStore.getState().fetchReport({ profile: 'alpha' });
  const firstRead = useGahStore.getState().report.data;
  await useGahStore.getState().fetchReport({ profile: 'alpha' });
  expect(firstRead).toEqual(data);
  expect(useGahStore.getState().report.data).toBe(firstRead);
  expect(getReport).toHaveBeenCalledTimes(1);
  expect(getReport).toHaveBeenCalledWith({ profile: 'alpha' });
});

test('concurrent readers make one request and see its result', async () => {
  const request = deferred<ReportData>();
  getReport.mockReturnValue(request.promise);
  const first = useGahStore.getState().fetchReport({ profile: 'alpha' });
  const second = useGahStore.getState().fetchReport({ profile: 'alpha' });
  expect(getReport).toHaveBeenCalledTimes(1);
  expect(useGahStore.getState().report.loading).toBe(true);
  request.resolve(report('alpha'));
  await Promise.all([first, second]);
  expect(useGahStore.getState().report).toMatchObject({ data: report('alpha'), loading: false, error: null });
});

test('refresh fetches again even while the previous result is fresh', async () => {
  getReport.mockResolvedValueOnce(report('alpha')).mockResolvedValueOnce(report('alpha', 2));
  await useGahStore.getState().fetchReport({ profile: 'alpha' });
  await useGahStore.getState().fetchReport({ profile: 'alpha' }, { force: true });
  expect(getReport).toHaveBeenCalledTimes(2);
  expect(useGahStore.getState().report.data).toEqual(report('alpha', 2));
});

test('a failed profile fetch exposes the error and clears the previous profile data', async () => {
  getReport.mockResolvedValueOnce(report('alpha')).mockRejectedValueOnce(new Error('provider unavailable'));
  await useGahStore.getState().fetchReport({ profile: 'alpha' });
  await useGahStore.getState().fetchReport({ profile: 'beta' });
  expect(getReport).toHaveBeenCalledTimes(2);
  expect(useGahStore.getState().report).toMatchObject({ data: null, loading: false, error: 'provider unavailable', fetchedAt: null });
});

test('a late response for the previous profile cannot overwrite the newer result', async () => {
  const oldRequest = deferred<ReportData>();
  const newRequest = deferred<ReportData>();
  getReport.mockReturnValueOnce(oldRequest.promise).mockReturnValueOnce(newRequest.promise);
  const oldFetch = useGahStore.getState().fetchReport({ profile: 'alpha' });
  const newFetch = useGahStore.getState().fetchReport({ profile: 'beta' });
  expect(getReport).toHaveBeenCalledTimes(2);
  newRequest.resolve(report('beta'));
  await newFetch;
  oldRequest.resolve(report('alpha'));
  await oldFetch;
  expect(useGahStore.getState().report).toMatchObject({ data: report('beta'), loading: false, error: null });
});
