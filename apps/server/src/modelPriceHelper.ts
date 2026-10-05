import { parseExtractedPrices, type PriceExtractor } from './modelPricing.js';
import { recordHelperUsage, runHelperTask } from './managerChat/helperTasks.js';

/**
 * Reads a pricing page with the low-cost helper model (GPT Luna on Codex by
 * default, or whatever Settings names as the helper). Only used for a page
 * whose price table could not be read directly; its rows are validated like
 * any other, and a helper that is off or fails simply yields none.
 */
export function helperPriceExtractor(profile: string): PriceExtractor {
  return async (_provider, page) => {
    const result = await runHelperTask({ profile, sourceBackend: 'codex', sourceBackendInstance: null, kind: 'price_table', input: page, fallback: { text: '' } });
    recordHelperUsage(profile, result);
    return result.generated ? parseExtractedPrices(result.text) : [];
  };
}
