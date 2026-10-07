import { workKey } from './workKey.js';

test.each(['42', '#0042', ' Ticket-0042 '])('normalizes issue identity %s', (input) => {
  expect(workKey(input)).toBe('#42');
});
test.each([['', ''], ['  CAFÉ-猫  ', 'café-猫'], ['ticket-nope', 'ticket-nope']])('preserves nonnumeric identity %s', (input, expected) => {
  expect(workKey(input)).toBe(expected);
});
