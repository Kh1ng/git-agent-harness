import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { DeviceAccessRequest } from '../../src/components/DeviceAccessRequest.js';
import type { PairingAccessRequest } from '@git-agent-harness/contracts';

function request(status: PairingAccessRequest['status'] = 'pending', expiresIn = 300_000): PairingAccessRequest {
  return { schema_version: 1, id: 'request-1', name: 'Linux laptop', matching_code: '123456', expires_at: new Date(Date.now() + expiresIn).toISOString(), server: { id: 'server-1', name: 'Home central', origin: 'https://central.example' }, access: 'Dashboard control', status };
}

test('restores a cookie-bound request, polls until denial, then stops', async ({ page, mount }) => {
  let calls = 0;
  await page.clock.install();
  await page.route('**/api/pairing/access/status', route => route.fulfill({ json: request(++calls === 1 ? 'pending' : 'denied') }));
  const component = await mount(<DeviceAccessRequest onPaired={async () => {}} />);
  await expect(component.getByText('123456', { exact: true })).toBeVisible();
  await expect(component.getByRole('status')).toContainText('Waiting for approval');
  await page.clock.fastForward(5000);
  await expect(component.getByRole('status')).toContainText('Access denied');
  await page.clock.fastForward(60_000);
  expect(calls).toBe(2);
  await component.getByRole('button', { name: 'Start a new request' }).click();
  await expect(component.getByLabel('Device name', { exact: true })).toBeVisible();
});

test('connection failure stops retries while the request still expires', async ({ page, mount }) => {
  let calls = 0;
  await page.clock.install();
  await page.route('**/api/pairing/access/status', route => ++calls === 1 ? route.fulfill({ json: request('pending', 10_000) }) : route.fulfill({ status: 503, json: { message: 'Server unavailable. Try again.' } }));
  const component = await mount(<DeviceAccessRequest onPaired={async () => {}} />);
  await expect(component.getByRole('status')).toContainText('Waiting for approval');
  await page.clock.fastForward(5000);
  await expect(component.getByRole('alert')).toContainText('Server unavailable');
  await page.clock.fastForward(8000);
  await expect(component.getByRole('status')).toContainText('This request expired');
  expect(calls).toBe(2);
});

test('approved access requires an explicit claim and retained matching device session', async ({ page, mount }) => {
  let claims = 0;
  let paired = false;
  await page.route('**/api/pairing/access/status', route => route.fulfill({ json: request('approved') }));
  await page.route('**/api/pairing/access/claim', route => { claims++; return route.fulfill({ json: { device: { id: 'device-1', name: 'Linux laptop' } } }); });
  await page.route('**/api/pairing/session', route => route.fulfill({ json: { principal: { kind: 'device', id: paired ? 'device-1' : 'wrong-device' } } }));
  const component = await mount(<DeviceAccessRequest onPaired={async () => { paired = true; }} />);
  await expect(component.getByRole('status')).toContainText('Access approved');
  expect(claims).toBe(0);
  await component.getByRole('button', { name: 'Continue to dashboard' }).click();
  await expect(component.getByRole('alert')).toContainText('did not retain its device session');
  expect(paired).toBe(false);
  expect(claims).toBe(1);
});
