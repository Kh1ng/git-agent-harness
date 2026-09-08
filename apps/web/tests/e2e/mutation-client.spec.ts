import { test, expect } from '@playwright/test';

test('mutation client sends fresh keys and distinguishes loop state from rejected operations', async ({ page }) => {
  // Load the real browser client without mounting the dashboard or calling a provider.
  await page.route('**/mutation-client-test', route => route.fulfill({ contentType: 'text/html', body: '<title>Client test</title>' }));
  await page.goto('/mutation-client-test');
  const requests: { path: string; key: string | undefined; method: string; body: unknown }[] = [];
  let status = 200;
  let body: object = { started: true };
  await page.route(`${new URL(page.url()).origin}/api/**`, route => {
    const request = route.request();
    requests.push({ path: new URL(request.url()).pathname, key: request.headers()['idempotency-key'], method: request.method(), body: request.postDataJSON() });
    return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
  });

  const call = (action: 'startLoop' | 'stopLoop' | 'setConfig') => page.evaluate(async action => {
    const modulePath = '/src/api/client.ts';
    const { gahApi, GahApiError }: typeof import('../../src/api/client.js') = await import(modulePath);
    try {
      return { result: await (action === 'setConfig' ? gahApi.setConfig({ current_manager: 'fixture' }) : gahApi[action]('fixture')) };
    } catch (error) {
      if (!(error instanceof GahApiError)) throw error;
      return { error: { message: error.message, status: error.status, endpoint: error.endpoint } };
    }
  }, action);

  expect(await call('startLoop')).toEqual({ result: { started: true } });
  status = 409;
  body = { started: false, alreadyRunning: true, pid: 123 };
  expect(await call('startLoop')).toEqual({ result: body });
  body = { stopped: false, error: 'No managed loop is running' };
  expect(await call('stopLoop')).toEqual({ result: body });

  for (const action of ['startLoop', 'stopLoop', 'setConfig'] as const) {
    for (const [httpStatus, error, message] of [
      [409, 'mutation_already_accepted', 'Refresh status before taking another action.'],
      [409, 'idempotency_conflict', 'This key was already used for different input.'],
      [400, 'idempotency_key_required', 'Supply an Idempotency-Key.'],
      [503, 'mutation_outcome_unknown', 'The operation may have completed. Refresh status.'],
    ] as const) {
      status = httpStatus;
      body = { error, message };
      const endpoint = action === 'setConfig' ? '/api/config' : `/api/loop/${action === 'startLoop' ? 'start' : 'stop'}`;
      expect(await call(action)).toEqual({ error: { message, status, endpoint } });
    }
  }
  expect(requests).toHaveLength(15); // One request per action, including failures; no automatic retry.
  for (const request of requests) {
    expect(request.method).toBe('POST');
    expect(request.key).toMatch(/^[a-f0-9]{32}$/);
    expect(request.body).toEqual(request.path === '/api/config' ? { current_manager: 'fixture' } : { profile: 'fixture' });
  }
  expect(new Set(requests.map(request => request.key)).size).toBe(requests.length);
});

test('Overview displays mutation failures instead of treating them as loop state', async ({ page }) => {
  await page.route('**/api/loop/status**', route => route.fulfill({ json: { running: false } }));
  let calls = 0;
  const message = 'The operation may have completed. Refresh status before taking another action.';
  await page.route('**/api/loop/start', route => {
    calls++;
    return route.fulfill({ status: 503, json: { error: 'mutation_outcome_unknown', message } });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Start loop' }).click();
  await expect(page.getByText(message, { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start loop' })).toBeEnabled();
  expect(calls).toBe(1);
});
