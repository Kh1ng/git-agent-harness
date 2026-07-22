import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  DEFAULT_BIND_HOST,
  InvalidBindHostError,
  isLoopbackBindHost,
  resolveBindHost,
  unauthenticatedExposureWarning,
  validateBindHost
} from './bindHost.js';

test('resolveBindHost defaults to 0.0.0.0 when HOST is unset', () => {
  assert.equal(resolveBindHost({}), '0.0.0.0');
  assert.equal(DEFAULT_BIND_HOST, '0.0.0.0');
});

test('resolveBindHost defaults to 0.0.0.0 when HOST is blank', () => {
  assert.equal(resolveBindHost({ HOST: '' }), '0.0.0.0');
  assert.equal(resolveBindHost({ HOST: '   ' }), '0.0.0.0');
});

test('resolveBindHost honors HOST=127.0.0.1', () => {
  assert.equal(resolveBindHost({ HOST: '127.0.0.1' }), '127.0.0.1');
});

test('resolveBindHost honors a specific interface address', () => {
  assert.equal(resolveBindHost({ HOST: '10.0.0.5' }), '10.0.0.5');
});

test('resolveBindHost trims surrounding whitespace', () => {
  assert.equal(resolveBindHost({ HOST: ' 192.168.1.10 \n' }), '192.168.1.10');
});

test('validateBindHost accepts the default, an override, and a specific interface', () => {
  assert.doesNotThrow(() => validateBindHost('0.0.0.0'));
  assert.doesNotThrow(() => validateBindHost('127.0.0.1'));
  assert.doesNotThrow(() => validateBindHost('10.0.0.5'));
  assert.doesNotThrow(() => validateBindHost('::1'));
});

test('validateBindHost rejects unusable bind values with a clear error', () => {
  assert.throws(() => validateBindHost('not-an-ip'), InvalidBindHostError);
  assert.throws(() => validateBindHost('localhost'), InvalidBindHostError);
  assert.throws(() => validateBindHost(''), InvalidBindHostError);
  try {
    validateBindHost('not-an-ip');
    assert.fail('expected validateBindHost to throw');
  } catch (error) {
    assert.ok(error instanceof InvalidBindHostError);
    assert.match((error as Error).message, /Invalid HOST bind address "not-an-ip"/);
  }
});

test('isLoopbackBindHost recognizes loopback addresses only', () => {
  assert.equal(isLoopbackBindHost('127.0.0.1'), true);
  assert.equal(isLoopbackBindHost('127.5.5.5'), true);
  assert.equal(isLoopbackBindHost('::1'), true);
  assert.equal(isLoopbackBindHost('0.0.0.0'), false);
  assert.equal(isLoopbackBindHost('10.0.0.5'), false);
});

test('unauthenticatedExposureWarning is silent on loopback', () => {
  assert.equal(unauthenticatedExposureWarning('127.0.0.1'), null);
  assert.equal(unauthenticatedExposureWarning('::1'), null);
});

test('unauthenticatedExposureWarning fires for the 0.0.0.0 default and other interfaces', () => {
  const defaultWarning = unauthenticatedExposureWarning('0.0.0.0');
  assert.ok(defaultWarning);
  assert.match(defaultWarning, /#532/);
  assert.match(defaultWarning, /0\.0\.0\.0/);

  const interfaceWarning = unauthenticatedExposureWarning('10.0.0.5');
  assert.ok(interfaceWarning);
  assert.match(interfaceWarning, /10\.0\.0\.5/);
});
