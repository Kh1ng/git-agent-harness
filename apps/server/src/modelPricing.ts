import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { LedgerUsage, ModelPrice, ModelPriceBook, ModelPriceChange, ModelPriceSourceStatus } from '@git-agent-harness/contracts';
import { stateBase } from './managerChat/chatSessions.js';

/** The providers' own pricing pages, as markdown. */
export const PRICE_SOURCES: { provider: string; url: string }[] = [
  { provider: 'openai', url: 'https://developers.openai.com/api/docs/pricing.md' },
  { provider: 'anthropic', url: 'https://platform.claude.com/docs/en/about-claude/pricing.md' }
];

export const PRICE_REFRESH_MS = 24 * 60 * 60 * 1000;
const HISTORY_LIMIT = 200;
/** No published per-million-token price is outside this; anything else is a misread. */
const MAX_PRICE = 2000;

export type PriceRow = Pick<ModelPrice, 'model' | 'input' | 'output' | 'cached_input' | 'cache_write'>;
/** Reads a page's prices with the low-cost helper model, for a page whose table could not be read. */
export type PriceExtractor = (provider: string, page: string) => Promise<PriceRow[]>;

export function priceBookPath(): string {
  return join(dirname(stateBase()), 'model-prices.json');
}

function emptyBook(): ModelPriceBook {
  return { checked_at: null, sources: [], prices: [], history: [] };
}

export function readPriceBook(path = priceBookPath()): ModelPriceBook {
  try {
    const book = JSON.parse(readFileSync(path, 'utf8')) as ModelPriceBook;
    return Array.isArray(book.prices) && Array.isArray(book.history) && Array.isArray(book.sources) ? book : emptyBook();
  } catch {
    return emptyBook();
  }
}

function writePriceBook(book: ModelPriceBook, path: string): void {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, JSON.stringify(book, null, 2));
  renameSync(temporary, path);
}

/** `$2.50 / MTok`, `$0.125`, `-` → a number, or null for a blank cell. */
function dollars(cell: string): number | null | undefined {
  const text = cell.replace(/[<>]/g, '').trim();
  if (!text || /^[-–—]$|^n\/a$/i.test(text)) return null;
  const match = /\$\s*([0-9]+(?:\.[0-9]+)?)/.exec(text);
  if (!match) return undefined;
  const value = Number(match[1]);
  return Number.isFinite(value) && value >= 0 && value <= MAX_PRICE ? value : undefined;
}

/** A model cell without its footnotes, links and notes in brackets. */
function modelName(cell: string): string {
  return cell.replace(/<sup>.*?<\/sup>/g, '').replace(/\[([^\]]*)\]\([^)]*\)/g, '$1').replace(/\(.*$/, '').replace(/[*`]/g, '').trim();
}

/** A row the page or a model gave is kept only if it is a sane price. */
export function validPriceRow(row: Partial<PriceRow> | null | undefined): row is PriceRow {
  const price = (value: unknown, optional = false) => (optional && value === null) || (typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= MAX_PRICE);
  return !!row && typeof row.model === 'string' && /^[\w .:\-]{2,80}$/.test(row.model) && price(row.input) && price(row.output)
    && price(row.cached_input, true) && price(row.cache_write, true) && (row.input! > 0 || row.output! > 0);
}

/**
 * Prices from a pricing page's markdown tables. A table counts when it has a
 * Model column, an input column and an output column; the first table that
 * names a model wins (standard prices come before batch and priority ones).
 */
export function parsePriceTables(markdown: string): PriceRow[] {
  const rows = new Map<string, PriceRow>();
  const lines = markdown.split('\n');
  for (let index = 0; index < lines.length; index++) {
    if (!lines[index].trim().startsWith('|') || !/^\s*\|[\s:|-]+\|\s*$/.test(lines[index + 1] ?? '')) continue;
    const header = lines[index].split('|').slice(1, -1).map((cell) => cell.replace(/[<>]/g, '').trim().toLowerCase());
    const model = header.findIndex((cell) => /^model/.test(cell));
    const column = (test: (cell: string) => boolean) => header.findIndex(test);
    const input = column((cell) => /input/.test(cell) && !/cach/.test(cell));
    const output = column((cell) => /output/.test(cell));
    const cached = column((cell) => /cached input|cache hits|cache read/.test(cell));
    const cacheWrite = column((cell) => /cache writes?/.test(cell));
    let cursor = index + 2;
    for (; cursor < lines.length && lines[cursor].trim().startsWith('|'); cursor++) {
      if (model === -1 || input === -1 || output === -1) continue;
      const cells = lines[cursor].split('|').slice(1, -1);
      const row = {
        model: modelName(cells[model] ?? ''),
        input: dollars(cells[input] ?? ''), output: dollars(cells[output] ?? ''),
        cached_input: cached === -1 ? null : dollars(cells[cached] ?? '') ?? null,
        cache_write: cacheWrite === -1 ? null : dollars(cells[cacheWrite] ?? '') ?? null
      };
      if (validPriceRow(row as Partial<PriceRow>) && !rows.has(row.model.toLowerCase())) rows.set(row.model.toLowerCase(), row as PriceRow);
    }
    index = cursor - 1;
  }
  return [...rows.values()];
}

/** The lines of a page worth showing a model: tables and anything with a dollar sign, bounded. */
export function pricePageDigest(page: string, limit = 24_000): string {
  return page.split('\n').filter((line) => line.includes('$') || line.trim().startsWith('|')).join('\n').slice(0, limit);
}

/** Rows from a helper model's reply: a JSON array, each row validated. */
export function parseExtractedPrices(reply: string): PriceRow[] {
  const start = reply.indexOf('[');
  const end = reply.lastIndexOf(']');
  if (start === -1 || end <= start) return [];
  try {
    const parsed = JSON.parse(reply.slice(start, end + 1)) as unknown[];
    if (!Array.isArray(parsed)) return [];
    return parsed.map((item) => {
      const row = item as Record<string, unknown>;
      return { model: String(row.model ?? '').trim(), input: row.input, output: row.output, cached_input: row.cached_input ?? null, cache_write: row.cache_write ?? null } as Partial<PriceRow>;
    }).filter(validPriceRow).slice(0, 200);
  } catch {
    return [];
  }
}

const FIELDS = ['input', 'output', 'cached_input', 'cache_write'] as const;

/**
 * Check every source and fold what changed into the stored book. A page that
 * cannot be fetched or read keeps its last known prices. When a page has no
 * readable table and an extractor is given, the helper model reads it; its
 * rows pass the same validation as table rows.
 */
export async function refreshModelPrices(options: {
  fetchText?: (url: string) => Promise<string>;
  extract?: PriceExtractor;
  now?: () => number;
  path?: string;
  sources?: { provider: string; url: string }[];
} = {}): Promise<ModelPriceBook> {
  const path = options.path ?? priceBookPath();
  const now = new Date((options.now ?? Date.now)()).toISOString();
  const fetchText = options.fetchText ?? (async (url: string) => {
    const response = await fetch(url, { headers: { accept: 'text/markdown, text/plain;q=0.9', 'user-agent': 'git-agent-harness price check' }, signal: AbortSignal.timeout(30_000), redirect: 'follow' });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return (await response.text()).slice(0, 2_000_000);
  });
  const book = readPriceBook(path);
  const history: ModelPriceChange[] = [];
  const sources: ModelPriceSourceStatus[] = [];
  let prices = [...book.prices];
  for (const source of options.sources ?? PRICE_SOURCES) {
    const status: ModelPriceSourceStatus = { provider: source.provider, url: source.url, checked_at: now, ok: false, method: null, rows: 0, error: null };
    sources.push(status);
    try {
      const page = await fetchText(source.url);
      let rows = parsePriceTables(page);
      status.method = 'table';
      if (rows.length === 0 && options.extract) {
        rows = (await options.extract(source.provider, pricePageDigest(page))).filter(validPriceRow);
        status.method = 'helper_model';
      }
      if (rows.length === 0) throw new Error('No price table found on the page');
      status.ok = true;
      status.rows = rows.length;
      const before = new Map(prices.filter((price) => price.provider === source.provider).map((price) => [price.model.toLowerCase(), price]));
      const next: ModelPrice[] = [];
      for (const row of rows) {
        const known = before.get(row.model.toLowerCase());
        before.delete(row.model.toLowerCase());
        if (!known) {
          // The first reading of a source is a baseline, not a list of changes.
          if (book.prices.some((price) => price.provider === source.provider)) history.push({ at: now, provider: source.provider, model: row.model, field: 'added', from: null, to: row.input });
          next.push({ provider: source.provider, ...row, source_url: source.url, changed_at: now });
          continue;
        }
        const changed = FIELDS.filter((field) => known[field] !== row[field]);
        for (const field of changed) history.push({ at: now, provider: source.provider, model: row.model, field, from: known[field], to: row[field] });
        next.push(changed.length > 0 ? { ...known, ...row, source_url: source.url, changed_at: now } : known);
      }
      for (const gone of before.values()) history.push({ at: now, provider: source.provider, model: gone.model, field: 'removed', from: gone.input, to: null });
      prices = [...prices.filter((price) => price.provider !== source.provider), ...next];
    } catch (error) {
      status.error = error instanceof Error ? error.message : String(error);
      const last = book.sources.find((candidate) => candidate.provider === source.provider);
      status.rows = last?.rows ?? 0;
    }
  }
  const updated: ModelPriceBook = { checked_at: now, sources, prices, history: [...history.reverse(), ...book.history].slice(0, HISTORY_LIMIT) };
  writePriceBook(updated, path);
  return updated;
}

function normal(name: string): string {
  return name.toLowerCase().replace(/\[.*?\]/g, '').replace(/^claude[\s-]+/, '').replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
}

/**
 * The price row for a model as the ledger names it. Exact names match first
 * (`gpt-6-sol`, `claude-sonnet-5-5` → "Claude Sonnet 5.5"). A bare family
 * alias (`sonnet`, `opus`) matches the newest model of that family, which
 * pricing pages list first.
 */
export function priceFor(model: string | null | undefined, prices: ModelPrice[]): ModelPrice | null {
  const wanted = normal(model ?? '');
  if (!wanted || wanted === 'default' || wanted === 'auto') return null;
  const exact = prices.find((price) => normal(price.model) === wanted);
  if (exact) return exact;
  if (/^(opus|sonnet|haiku|fable|mythos)$/.test(wanted)) return prices.find((price) => normal(price.model).startsWith(`${wanted}-`)) ?? null;
  return null;
}

/**
 * What an attempt's tokens would cost at a published API price. OpenAI
 * counts cached tokens inside input tokens; Anthropic reports them apart.
 * Null without an input and output breakdown.
 */
export function apiEquivalentUsd(usage: Pick<LedgerUsage, never> & { input_tokens?: number | null; output_tokens?: number | null; cache_read_tokens?: number | null; cache_write_tokens?: number | null } | null | undefined, price: ModelPrice): number | null {
  if (!usage || typeof usage.input_tokens !== 'number' || typeof usage.output_tokens !== 'number') return null;
  const cacheRead = usage.cache_read_tokens ?? 0;
  const cacheWrite = usage.cache_write_tokens ?? 0;
  const freshInput = price.provider === 'openai' ? Math.max(0, usage.input_tokens - cacheRead) : usage.input_tokens;
  const dollarsTotal = freshInput * price.input + usage.output_tokens * price.output
    + cacheRead * (price.cached_input ?? price.input) + cacheWrite * (price.cache_write ?? price.input);
  return dollarsTotal / 1_000_000;
}

/** Check once at start when the book is missing or a day old, then daily. */
export function startModelPriceRefresh(extract?: PriceExtractor, log: (message: string) => void = () => {}): () => void {
  const run = () => {
    refreshModelPrices({ extract }).then((book) => {
      const failed = book.sources.filter((source) => !source.ok);
      log(`Model prices checked: ${book.prices.length} models${failed.length ? `; failed: ${failed.map((source) => `${source.provider} (${source.error})`).join(', ')}` : ''}`);
    }).catch((error) => log(`Model price check failed: ${error instanceof Error ? error.message : String(error)}`));
  };
  const last = Date.parse(readPriceBook().checked_at ?? '');
  const firstDelay = Number.isFinite(last) ? Math.max(5_000, PRICE_REFRESH_MS - (Date.now() - last)) : 5_000;
  let interval: ReturnType<typeof setInterval> | undefined;
  const first = setTimeout(() => { run(); interval = setInterval(run, PRICE_REFRESH_MS); interval.unref?.(); }, firstDelay);
  first.unref?.();
  return () => { clearTimeout(first); if (interval) clearInterval(interval); };
}
