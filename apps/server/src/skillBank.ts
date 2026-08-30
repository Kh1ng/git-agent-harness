/**
 * Versioned skill bank on the central node (issue #964, parent #963).
 *
 * A skill is authored and installed ONCE here and bound to any backend on any
 * node without touching that provider's own configuration. The bank lives on
 * the central node only, per the central/worker contract in #938.
 *
 * Durability follows `apps/server/src/projectCatalog.ts`'s established
 * pattern rather than inventing a new one: atomic temp-file + rename on
 * write, schema-validated on read with a clear error naming the file and the
 * problem on a malformed file -- never a silent empty bank.
 *
 * Versions coexist: `putSkill` upserts one (id, version) record; reads
 * resolve to the newest version unless a version is named explicitly. A skill
 * with live bindings (tracked here as `bindings[skillId]`, populated by #965)
 * cannot be deleted -- deletion is refused with the binding labels, never
 * silently orphaned.
 */

import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { homedir } from 'node:os';
import type { Skill, SkillBankFile, SkillSummary } from '@git-agent-harness/contracts';

function bankPath(): string {
  return process.env.GAH_SKILL_BANK_PATH
    || resolve(process.env.XDG_CONFIG_HOME || resolve(homedir(), '.config'), 'gah/skills.json');
}

function emptyBank(): SkillBankFile {
  return { skills: [], bindings: {} };
}

function readBank(): SkillBankFile {
  const path = bankPath();
  if (!existsSync(path)) return emptyBank();
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(path, 'utf8'));
  } catch (error) {
    throw new Error(
      `Invalid skill bank at ${path}: ${error instanceof Error ? error.message : String(error)}`
    );
  }
  if (
    typeof parsed !== 'object'
    || parsed === null
    || !Array.isArray((parsed as { skills?: unknown }).skills)
  ) {
    throw new Error(`Invalid skill bank at ${path}: expected an object with a "skills" array`);
  }
  const skills = (parsed as { skills: unknown[] }).skills;
  for (const [index, skill] of skills.entries()) {
    if (typeof skill !== 'object' || skill === null || Array.isArray(skill)) {
      throw new Error(`Invalid skill bank at ${path}: skills[${index}] must be an object`);
    }
    const record = skill as Record<string, unknown>;
    for (const field of ['id', 'version', 'displayName', 'description', 'content', 'source'] as const) {
      if (typeof record[field] !== 'string') {
        throw new Error(`Invalid skill bank at ${path}: skills[${index}].${field} must be a string`);
      }
    }
    if (!Array.isArray(record.backends)) {
      throw new Error(`Invalid skill bank at ${path}: skills[${index}].backends must be an array`);
    }
    for (const [backendIndex, backend] of record.backends.entries()) {
      if (typeof backend !== 'string') {
        throw new Error(
          `Invalid skill bank at ${path}: skills[${index}].backends[${backendIndex}] must be a string`
        );
      }
    }
    for (const field of ['createdAt', 'updatedAt'] as const) {
      if (typeof record[field] !== 'number' || !Number.isFinite(record[field])) {
        throw new Error(`Invalid skill bank at ${path}: skills[${index}].${field} must be a finite number`);
      }
    }
  }

  const bindings = (parsed as { bindings?: unknown }).bindings;
  if (typeof bindings !== 'object' || bindings === null || Array.isArray(bindings)) {
    throw new Error(`Invalid skill bank at ${path}: bindings must be an object`);
  }
  for (const [skillId, labels] of Object.entries(bindings)) {
    if (!Array.isArray(labels)) {
      throw new Error(`Invalid skill bank at ${path}: bindings.${skillId} must be an array`);
    }
    for (const [labelIndex, label] of labels.entries()) {
      if (typeof label !== 'string') {
        throw new Error(
          `Invalid skill bank at ${path}: bindings.${skillId}[${labelIndex}] must be a string`
        );
      }
    }
  }

  return { skills: skills as Skill[], bindings: bindings as Record<string, string[]> };
}

function writeBank(file: SkillBankFile): void {
  const path = bankPath();
  mkdirSync(dirname(path), { recursive: true });
  const temporaryPath = `${path}.${process.pid}.tmp`;
  writeFileSync(temporaryPath, `${JSON.stringify(file, null, 2)}\n`);
  renameSync(temporaryPath, path);
}

function newestVersion(a: Skill, b: Skill): number {
  const parse = (version: string) => {
    const withoutBuild = version.split('+', 1)[0];
    const separator = withoutBuild.indexOf('-');
    const core = separator >= 0 ? withoutBuild.slice(0, separator) : withoutBuild;
    const parts = core.split('.');
    if (parts.some((part) => !/^\d+$/.test(part))) return null;
    return {
      core: parts.map((part) => Number.parseInt(part, 10)),
      prerelease: separator >= 0 ? withoutBuild.slice(separator + 1).split('.') : null
    };
  };
  const aParsed = parse(a.version);
  const bParsed = parse(b.version);
  if (!aParsed || !bParsed) return a.version.localeCompare(b.version);

  for (let index = 0; index < Math.max(aParsed.core.length, bParsed.core.length); index++) {
    const aNum = aParsed.core[index] ?? 0;
    const bNum = bParsed.core[index] ?? 0;
    if (aNum !== bNum) return aNum > bNum ? 1 : -1;
  }

  if (aParsed.prerelease === null && bParsed.prerelease !== null) return 1;
  if (aParsed.prerelease !== null && bParsed.prerelease === null) return -1;
  if (aParsed.prerelease && bParsed.prerelease) {
    for (let index = 0; index < Math.min(aParsed.prerelease.length, bParsed.prerelease.length); index++) {
      const aPart = aParsed.prerelease[index];
      const bPart = bParsed.prerelease[index];
      if (aPart === bPart) continue;
      const aNumeric = /^\d+$/.test(aPart);
      const bNumeric = /^\d+$/.test(bPart);
      if (aNumeric && bNumeric) return Number(aPart) > Number(bPart) ? 1 : -1;
      if (aNumeric !== bNumeric) return aNumeric ? -1 : 1;
      return aPart > bPart ? 1 : -1;
    }
    if (aParsed.prerelease.length !== bParsed.prerelease.length) {
      return aParsed.prerelease.length > bParsed.prerelease.length ? 1 : -1;
    }
  }
  return a.version.localeCompare(b.version);
}

/** Every versioned record, newest-first within each id. */
export function listSkills(): Skill[] {
  const { skills } = readBank();
  return [...skills].sort((a, b) => {
    if (a.id !== b.id) return a.id.localeCompare(b.id);
    return newestVersion(b, a);
  });
}

/** Resolve one skill: the newest version unless a version is named. */
export function getSkill(id: string, version?: string): Skill | null {
  const candidates = listSkills().filter((skill) => skill.id === id);
  if (candidates.length === 0) return null;
  if (version) return candidates.find((skill) => skill.version === version) ?? null;
  return candidates[0];
}

/** Upsert one (id, version) record. Other versions of the same id survive. */
export function putSkill(skill: Skill): Skill {
  const file = readBank();
  const index = file.skills.findIndex(
    (existing) => existing.id === skill.id && existing.version === skill.version
  );
  if (index >= 0) file.skills[index] = skill;
  else file.skills.push(skill);
  writeBank(file);
  return skill;
}

/** Bind a skill id to a backend instance label (#965 populates these). */
export function addBinding(skillId: string, label: string): void {
  const file = readBank();
  const bindings = file.bindings[skillId] ?? [];
  if (!bindings.includes(label)) bindings.push(label);
  file.bindings[skillId] = bindings;
  writeBank(file);
}

/** Remove a binding label. */
export function removeBinding(skillId: string, label: string): void {
  const file = readBank();
  const bindings = file.bindings[skillId] ?? [];
  file.bindings[skillId] = bindings.filter((existing) => existing !== label);
  writeBank(file);
}

export function listBindings(skillId: string): string[] {
  return readBank().bindings[skillId] ?? [];
}

/** Delete every version of a skill id. Refused (with the binding labels) when
 * the skill is still bound -- otherwise the bound backend would silently
 * resolve to nothing (issue #964 AC7). */
export function deleteSkill(id: string): Skill[] {
  const file = readBank();
  const bindings = file.bindings[id] ?? [];
  if (bindings.length > 0) {
    throw new Error(
      `Cannot delete skill '${id}': it is still bound to ${bindings.join(', ')}; unbind first`
    );
  }
  const remaining = file.skills.filter((skill) => skill.id !== id);
  const removed = file.skills.filter((skill) => skill.id === id);
  if (remaining.length !== file.skills.length) {
    writeBank({ ...file, skills: remaining });
  }
  return removed;
}

/** Seed the bank from the repo's docs (issue #964 AC5): the one skill that
 * exists today becomes a first-class record. Idempotent -- never overwrites a
 * manually-edited version. Returns the seeded record, or null if already
 * present or the source doc is missing. */
export function seedSkillFromDocs(
  docsPath: string,
  sourceLabel: string,
  skillId: string,
  version: string,
  displayName: string,
  description: string,
  backends: string[]
): Skill | null {
  if (!existsSync(docsPath)) return null;
  if (getSkill(skillId, version)) return null;
  const content = readFileSync(docsPath, 'utf8');
  const now = Date.now();
  return putSkill({
    id: skillId,
    version,
    displayName,
    description,
    content,
    backends,
    source: sourceLabel,
    createdAt: now,
    updatedAt: now
  });
}

/** `listSkills()` plus the bound flag, for API consumers that don't need the
 * full content. */
export function listSkillSummaries(): SkillSummary[] {
  return listSkills().map((skill) => ({
    id: skill.id,
    version: skill.version,
    displayName: skill.displayName,
    description: skill.description,
    backends: skill.backends,
    source: skill.source,
    bound: listBindings(skill.id).length > 0
  }));
}
