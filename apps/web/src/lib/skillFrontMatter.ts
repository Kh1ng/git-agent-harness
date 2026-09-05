/**
 * Reads the `---`-delimited front matter block of an uploaded SKILL.md file
 * into the JSON shape POST /api/skills already accepts (see
 * apps/server/src/server.ts and packages/contracts/src/gah.ts's `Skill`).
 *
 * Deliberately a hand-rolled `key: value` reader, not a YAML parser -- the
 * skill bank has no YAML dependency and this doesn't need one either. Each
 * line's *first* colon separates key from value so values containing colons
 * (e.g. a description like "Role: GAH Manager") survive intact.
 */
import type { SkillCreateData } from '@git-agent-harness/contracts';

export class SkillFrontMatterError extends Error {}

const DEFAULT_VERSION = '1.0.0';

function unquote(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed.length >= 2) {
    const first = trimmed[0];
    const last = trimmed[trimmed.length - 1];
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      return trimmed.slice(1, -1);
    }
  }
  return trimmed;
}

function parseList(raw: string): string[] {
  const trimmed = raw.trim();
  const unbracketed = trimmed.startsWith('[') && trimmed.endsWith(']') ? trimmed.slice(1, -1) : trimmed;
  return unbracketed
    .split(',')
    .map((item) => unquote(item))
    .filter((item) => item.length > 0);
}

/** Parses the leading front matter block only; returns `{}` when the file
 * has none (an unfenced markdown file is not an error here -- the caller
 * decides whether the resulting fields are sufficient). */
export function parseFrontMatterFields(text: string): Record<string, string> {
  const lines = text.split(/\r?\n/);
  if (lines[0]?.trim() !== '---') return {};
  const closingIndex = lines.findIndex((line, index) => index > 0 && line.trim() === '---');
  if (closingIndex === -1) return {};
  const fields: Record<string, string> = {};
  let i = 1;
  while (i < closingIndex) {
    const line = lines[i];
    const separator = line.indexOf(':');
    if (separator === -1) { i++; continue; }
    const key = line.slice(0, separator).trim();
    if (!key) { i++; continue; }
    const rawValue = line.slice(separator + 1).trim();
    // ponytail: handles | and |- only; add > (folded) if real skills use it
    if (rawValue === '|' || rawValue === '|-' || rawValue === '|+') {
      const blockLines: string[] = [];
      let indent = -1;
      i++;
      while (i < closingIndex) {
        const nextLine = lines[i];
        if (nextLine.trim() === '') { blockLines.push(''); i++; continue; }
        const lineIndent = nextLine.length - nextLine.trimStart().length;
        if (indent === -1) indent = lineIndent;
        if (lineIndent < indent) break;
        blockLines.push(nextLine.slice(indent));
        i++;
      }
      while (blockLines.length > 0 && blockLines[blockLines.length - 1] === '') blockLines.pop();
      fields[key] = blockLines.join('\n');
      continue;
    }
    fields[key] = rawValue;
    i++;
  }
  return fields;
}

/** Builds the POST /api/skills body from an uploaded SKILL.md. The complete
 * file (front matter included) is preserved verbatim as `content`. Throws
 * `SkillFrontMatterError` when the front matter has no usable id -- that's a
 * validation error the caller should show inline, not send to the API. */
export function skillFromFrontMatter(fileName: string, content: string): SkillCreateData {
  const fields = parseFrontMatterFields(content);
  const id = unquote(fields.id ?? fields.name ?? '');
  if (!id) {
    throw new SkillFrontMatterError(
      `${fileName} has no "id" (or "name") in its front matter -- add a leading "---" block with an id: line.`
    );
  }
  const version = unquote(fields.version ?? '') || DEFAULT_VERSION;
  const displayName = unquote(fields.displayName ?? fields.name ?? '');
  const description = unquote(fields.description ?? '');
  const source = unquote(fields.source ?? '') || fileName;
  return {
    id,
    version,
    content,
    ...(displayName ? { displayName } : {}),
    ...(description ? { description } : {}),
    ...(fields.backends ? { backends: parseList(fields.backends) } : {}),
    source
  };
}
