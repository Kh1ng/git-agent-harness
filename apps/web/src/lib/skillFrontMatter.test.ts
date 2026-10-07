import { parseFrontMatterFields, skillFromFrontMatter, SkillFrontMatterError } from './skillFrontMatter.js';

test('preserves content and parses quotes, lists, colons and unicode', () => {
  const content = '---\r\nid: "worker"\r\nversion: 2.0\r\ndisplayName: "猫"\r\ndescription: Role: Manager\r\nbackends: ["codex", \'claude\', ,]\r\nsource: custom\r\n---\r\n# Body';
  expect(skillFromFrontMatter('SKILL.md', content)).toEqual({ id: 'worker', version: '2.0', displayName: '猫', description: 'Role: Manager', backends: ['codex', 'claude'], source: 'custom', content });
});
test('reads multiline descriptions and skips malformed field lines', () => {
  expect(parseFrontMatterFields('---\nno separator\n: ignored\nname: helper\ndescription: |-\n  first\n  second\n\nversion: 3\n---')).toEqual({ name: 'helper', description: 'first\nsecond', version: '3' });
});
test('defaults version and source and accepts name as identity', () => {
  const content = "---\nname: 'helper'\nversion: ''\n---\nBody";
  expect(skillFromFrontMatter('helper.md', content)).toEqual({ id: 'helper', displayName: 'helper', version: '1.0.0', source: 'helper.md', content });
});
test.each(['', '# Body', '---\nid: missing-close', '---\nid: ""\n---'])('rejects missing or unusable identity in %s', (content) => {
  expect(() => skillFromFrontMatter('bad.md', content)).toThrow(SkillFrontMatterError);
  expect(() => skillFromFrontMatter('bad.md', content)).toThrow('bad.md has no "id"');
});
test('returns no fields for an absent or unterminated leading block', () => {
  expect(parseFrontMatterFields('Body\n---\nid: later\n---')).toEqual({});
  expect(parseFrontMatterFields('---\nid: incomplete')).toEqual({});
});
