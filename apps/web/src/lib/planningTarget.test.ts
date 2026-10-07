import { choiceTarget, targetChoice, nodeName, validMapSlug } from './planningTarget.js';

test('round trips epic and file selections', () => {
  expect(choiceTarget('epic:900')).toEqual({ epic: 900 });
  expect(targetChoice({ epic: 900 })).toBe('epic:900');
  expect(choiceTarget('file:node-handoff')).toEqual({ file: 'node-handoff' });
  expect(targetChoice({ file: 'node-handoff' })).toBe('file:node-handoff');
});
test('labels issue numbers and zero-padded file tickets', () => {
  expect(nodeName({}, 2)).toBe('#2');
  expect(nodeName({ file: 'plan' }, 2)).toBe('02');
  expect(nodeName({ file: 'plan' }, 123)).toBe('123');
  expect(nodeName({ file: 'plan' }, 0)).toBe('00');
});
test.each(['a', 'node-handoff', 'a'.repeat(100)])('accepts slug %s', (slug) => expect(validMapSlug(slug)).toBe(true));
test.each(['', '-start', '../plan', 'UPPER', '猫', 'a'.repeat(101)])('rejects malformed slug %s', (slug) => expect(validMapSlug(slug)).toBe(false));
