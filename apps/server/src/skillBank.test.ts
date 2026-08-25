import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Skill } from '@git-agent-harness/contracts';
import {
  addBinding,
  deleteSkill,
  getSkill,
  listBindings,
  listSkills,
  putSkill,
  removeBinding,
  seedSkillFromDocs
} from './skillBank.js';

function withTempBank(testFn: () => void): void {
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-'));
  process.env.GAH_SKILL_BANK_PATH = join(dir, 'skills.json');
  try {
    testFn();
  } finally {
    delete process.env.GAH_SKILL_BANK_PATH;
    rmSync(dir, { recursive: true, force: true });
  }
}

function skill(id: string, version: string, content = 'content'): Skill {
  return {
    id,
    version,
    displayName: id,
    description: `${id} v${version}`,
    content,
    backends: ['hermes'],
    source: 'test',
    createdAt: 1000,
    updatedAt: 1000
  };
}

test('a skill survives a write + re-read (restart) byte-identically', () => {
  withTempBank(() => {
    putSkill(skill('alpha', '1.0.0', 'the skill text\nwith newlines'));
    const reread = getSkill('alpha', '1.0.0');
    assert.equal(reread?.content, 'the skill text\nwith newlines');
    assert.equal(reread?.version, '1.0.0');
    assert.equal(getSkill('missing'), null);
  });
});

test('two versions coexist and unversioned reads resolve to the newest', () => {
  withTempBank(() => {
    putSkill(skill('alpha', '1.0.0', 'v1'));
    putSkill(skill('alpha', '2.0.0', 'v2'));
    putSkill(skill('alpha', '1.5.0', 'v1.5'));
    assert.equal(getSkill('alpha')?.version, '2.0.0');
    assert.equal(getSkill('alpha', '1.0.0')?.content, 'v1');
    assert.equal(getSkill('alpha', '1.5.0')?.content, 'v1.5');
    assert.equal(listSkills().filter((s) => s.id === 'alpha').length, 3);
  });
});

test('a corrupted bank file produces a clear error naming the file, never an empty bank', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-'));
  const path = join(dir, 'skills.json');
  process.env.GAH_SKILL_BANK_PATH = path;
  try {
    writeFileSync(path, '{not valid json', 'utf8');
    assert.throws(() => listSkills(), /Invalid skill bank at .*skills\.json/);
    writeFileSync(path, JSON.stringify({ skills: [{ id: 'x' }] }), 'utf8');
    assert.throws(() => listSkills(), /missing id\/version\/content/);
  } finally {
    delete process.env.GAH_SKILL_BANK_PATH;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('concurrent writes never interleave into a partial file', async () => {
  withTempBank(() => {
    // Serialize enough writes to make a torn-write regression observable: the
    // atomic temp-file + rename guarantees the final file is always one
    // complete JSON document regardless of ordering.
    const writes = Array.from({ length: 50 }, (_, index) =>
      Promise.resolve().then(() => putSkill(skill('bulk', `${index}.0.0`, `bulk-${index}`)))
    );
    return Promise.all(writes).then(() => {
      const skills = listSkills();
      assert.equal(skills.length, 50);
      assert.ok(skills.every((s) => /^bulk-/.test(s.content)));
    });
  });
});

test('a skill bound to a backend cannot be deleted; the error names the bindings', () => {
  withTempBank(() => {
    putSkill(skill('alpha', '1.0.0'));
    addBinding('alpha', 'hermes:gah');
    assert.deepEqual(listBindings('alpha'), ['hermes:gah']);
    assert.throws(() => deleteSkill('alpha'), /still bound to hermes:gah/);
    assert.equal(getSkill('alpha', '1.0.0')?.content, 'content', 'bound skill still present');
    removeBinding('alpha', 'hermes:gah');
    const removed = deleteSkill('alpha');
    assert.equal(removed.length, 1);
    assert.equal(getSkill('alpha'), null);
  });
});

test('seed from docs makes the doc a first-class record, idempotently', () => {
  withTempBank(() => {
    const dir = mkdtempSync(join(tmpdir(), 'gah-skills-'));
    const docPath = join(dir, 'gah-manager-skill.md');
    writeFileSync(docPath, '# Role: GAH Manager\n\ncontent here\n', 'utf8');
    try {
      const seeded = seedSkillFromDocs(docPath, 'docs/gah-manager-skill.md', 'gah-manager', '1.0.0', 'GAH Manager', 'desc', ['hermes']);
      assert.equal(seeded?.content, '# Role: GAH Manager\n\ncontent here\n');
      // Idempotent: a second seed never overwrites.
      writeFileSync(docPath, '# changed', 'utf8');
      assert.equal(seedSkillFromDocs(docPath, 'docs/gah-manager-skill.md', 'gah-manager', '1.0.0', 'GAH Manager', 'desc', ['hermes']), null);
      assert.equal(getSkill('gah-manager', '1.0.0')?.content, '# Role: GAH Manager\n\ncontent here\n');
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
