import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { spawn } from 'node:child_process';
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

/** Sets a per-test bank path, runs the body, then restores the environment.
 * MUST await the body: a synchronous finally would delete the env var while
 * async writes are still in flight, sending them to the real default bank
 * (~/.config/gah/skills.json) -- which is how the concurrent-writes test once
 * polluted a production node with 50 `bulk` skills. */
async function withTempBank(testFn: () => void | Promise<void>): Promise<void> {
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-'));
  process.env.GAH_SKILL_BANK_PATH = join(dir, 'skills.json');
  try {
    await testFn();
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

test('a skill survives a write + re-read (restart) byte-identically', async () => {
  await withTempBank(() => {
    putSkill(skill('alpha', '1.0.0', 'the skill text\nwith newlines'));
    const reread = getSkill('alpha', '1.0.0');
    assert.equal(reread?.content, 'the skill text\nwith newlines');
    assert.equal(reread?.version, '1.0.0');
    assert.equal(getSkill('missing'), null);
  });
});

test('two versions coexist and unversioned reads resolve to the newest', async () => {
  await withTempBank(() => {
    putSkill(skill('alpha', '1.0.0', 'v1'));
    putSkill(skill('alpha', '2.0.0', 'v2'));
    putSkill(skill('alpha', '1.5.0', 'v1.5'));
    assert.equal(getSkill('alpha')?.version, '2.0.0');
    assert.equal(getSkill('alpha', '1.0.0')?.content, 'v1');
    assert.equal(getSkill('alpha', '1.5.0')?.content, 'v1.5');
    assert.equal(listSkills().filter((s) => s.id === 'alpha').length, 3);

    putSkill(skill('release', '1.0.0-rc.1', 'prerelease'));
    putSkill(skill('release', '1.0.0', 'release'));
    assert.equal(getSkill('release')?.content, 'release');
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
    assert.throws(() => listSkills(), /skills\[0\]\.version must be a string/);
  } finally {
    delete process.env.GAH_SKILL_BANK_PATH;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a persisted bank validates every skill field and the complete bindings shape', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-'));
  const path = join(dir, 'skills.json');
  process.env.GAH_SKILL_BANK_PATH = path;
  const validSkill = skill('alpha', '1.0.0');
  const invalidBanks: Array<[string, unknown]> = [
    ['skills[0].id', { skills: [{ ...validSkill, id: 1 }], bindings: {} }],
    ['skills[0].version', { skills: [{ ...validSkill, version: null }], bindings: {} }],
    ['skills[0].displayName', { skills: [{ ...validSkill, displayName: false }], bindings: {} }],
    ['skills[0].description', { skills: [{ ...validSkill, description: [] }], bindings: {} }],
    ['skills[0].content', { skills: [{ ...validSkill, content: {} }], bindings: {} }],
    ['skills[0].backends', { skills: [{ ...validSkill, backends: 'hermes' }], bindings: {} }],
    ['skills[0].backends[0]', { skills: [{ ...validSkill, backends: [1] }], bindings: {} }],
    ['skills[0].source', { skills: [{ ...validSkill, source: 1 }], bindings: {} }],
    ['skills[0].createdAt', { skills: [{ ...validSkill, createdAt: 'now' }], bindings: {} }],
    ['skills[0].updatedAt', { skills: [{ ...validSkill, updatedAt: null }], bindings: {} }],
    ['bindings', { skills: [validSkill], bindings: [] }],
    ['bindings.alpha', { skills: [validSkill], bindings: { alpha: 'hermes:gah' } }],
    ['bindings.alpha[0]', { skills: [validSkill], bindings: { alpha: [1] } }]
  ];

  try {
    for (const [field, bank] of invalidBanks) {
      writeFileSync(path, JSON.stringify(bank), 'utf8');
      assert.throws(
        () => listSkills(),
        (error: unknown) => error instanceof Error
          && error.message.includes(`Invalid skill bank at ${path}`)
          && error.message.includes(field),
        field
      );
    }
  } finally {
    delete process.env.GAH_SKILL_BANK_PATH;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('concurrent writes never interleave into a partial file', async () => {
  await withTempBank(async () => {
    const path = process.env.GAH_SKILL_BANK_PATH!;
    putSkill(skill('baseline', '1.0.0'));
    const skillBankModule = new URL('./skillBank.ts', import.meta.url).href;
    const childScript = `
      import { putSkill } from ${JSON.stringify(skillBankModule)};
      const index = process.argv[1];
      putSkill({
        id: 'bulk',
        version: index + '.0.0',
        displayName: 'bulk',
        description: 'child ' + index,
        content: 'x'.repeat(256 * 1024) + index,
        backends: ['hermes'],
        source: 'child-test',
        createdAt: 1000,
        updatedAt: 1000
      });
    `;

    let writersRunning = true;
    const writers = Promise.all(Array.from({ length: 12 }, (_, index) => new Promise<void>((resolve, reject) => {
      const child = spawn(
        process.execPath,
        ['--import', 'tsx', '--input-type=module', '--eval', childScript, String(index)],
        { env: { ...process.env, GAH_SKILL_BANK_PATH: path }, stdio: ['ignore', 'ignore', 'pipe'] }
      );
      let stderr = '';
      child.stderr.setEncoding('utf8');
      child.stderr.on('data', (chunk: string) => { stderr += chunk; });
      child.on('error', reject);
      child.on('close', (code) => {
        if (code === 0) resolve();
        else reject(new Error(`skill writer exited ${code}: ${stderr}`));
      });
    }))).finally(() => { writersRunning = false; });
    const reader = (async () => {
      while (writersRunning) {
        listSkills();
        await new Promise<void>((resolve) => setImmediate(resolve));
      }
    })();
    const results = await Promise.allSettled([writers, reader]);
    for (const result of results) {
      if (result.status === 'rejected') throw result.reason;
    }

    assert.doesNotThrow(() => JSON.parse(readFileSync(path, 'utf8')));
    const skills = listSkills();
    assert.ok(skills.some((stored) => stored.id === 'baseline'));
    assert.ok(skills.some((stored) => stored.id === 'bulk'));
    assert.ok(skills.filter((stored) => stored.id === 'bulk').every((stored) => stored.content.length >= 256 * 1024));
  });
});

test('a skill bound to a backend cannot be deleted; the error names the bindings', async () => {
  await withTempBank(() => {
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

test('seed from docs makes the doc a first-class record, idempotently', async () => {
  await withTempBank(() => {
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
