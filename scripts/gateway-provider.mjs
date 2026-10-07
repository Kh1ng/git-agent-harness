// Write the selected model provider into a MemoryCore tdai-gateway.local.yaml.
// Shared by install-linux.sh, install-macos.sh, and
// validate-gateway-provider.sh so the three cannot drift apart.
//
//   node scripts/gateway-provider.mjs <memorycore> <config> <provider> \
//     [endpoint] [llm-model] [embedding-model] [dimensions] \
//     [embedding-key-given] [llm-key-given] [embedding-key-stored]
//
// Supported contract (Kh1ng/TencentDB-Agent-Memory MemoryCore, src/config.ts
// and src/gateway/config.ts): every embedding provider except none/local/qclaw
// is an OpenAI-compatible service that posts to `${baseUrl}/embeddings` and is
// disabled unless apiKey, baseUrl, model, and dimensions are all set. The
// generation LLM is used only with a non-empty llm.apiKey; a non-empty
// TDAI_LLM_API_KEY in the gateway environment overrides it.
//
// Credentials never pass through here: the last three arguments only say
// whether a key was given or is already stored. Prints, one word per line, what the installer has
// to act on:
//   llm-endpoint-changed        a stored generation key belongs elsewhere
//   embedding-endpoint-changed  a stored embedding key belongs elsewhere
import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

const [memoryCore, configPath, provider, endpoint, llmModel, embedModel, dimensionsArg, embeddingKeyGiven, llmKeyGiven, embeddingKeyStored] = process.argv.slice(2);
// The checkout's own YAML library, so comments and unrelated settings survive.
const yaml = createRequire(resolve(memoryCore, 'package.json'))('yaml');

const providers = {
  ollama: { baseUrl: 'http://127.0.0.1:11434/v1', llmModel: 'llama3', embedModel: 'nomic-embed-text' },
  openai: { baseUrl: 'https://api.openai.com/v1', llmModel: 'gpt-4o', embedModel: 'text-embedding-3-small' },
};
const defaults = Object.hasOwn(providers, provider) ? providers[provider] : undefined;
if (!defaults) {
  console.error(`ERROR: GAH_GATEWAY_PROVIDER must be openai or ollama, not '${provider}'.`);
  process.exit(1);
}
const knownDimensions = new Map([
  ['nomic-embed-text', 768], ['mxbai-embed-large', 1024], ['all-minilm', 384],
  ['text-embedding-3-small', 1536], ['text-embedding-3-large', 3072], ['text-embedding-ada-002', 1536],
]);
const baseUrl = endpoint || defaults.baseUrl;
const model = embedModel || defaults.embedModel;
const dimensions = dimensionsArg ? Number(dimensionsArg) : knownDimensions.get(model);
if (!Number.isInteger(dimensions) || dimensions <= 0) {
  console.error(`ERROR: set GAH_GATEWAY_EMBEDDING_DIMENSIONS to the vector size of embedding model '${model}'.`);
  process.exit(1);
}

// Ollama and other keyless local servers ignore bearer credentials, but the
// gateway calls a backend only with a non-empty key. This literal is not a
// secret; it is the value Ollama's own OpenAI-compatibility guide uses.
const KEYLESS = 'ollama';
const doc = yaml.parseDocument(readFileSync(configPath, 'utf8'));
const moved = (path) => {
  const previous = doc.getIn(path);
  return Boolean(previous) && previous !== baseUrl;
};
const llmMoved = moved(['llm', 'baseUrl']);
const embeddingMoved = moved(['memory', 'embedding', 'baseUrl']);

/** A given key is read from the gateway env file; otherwise keep what still applies. */
function apiKey(path, variable, given, endpointMoved) {
  if (given) return `\${${variable}}`;
  const existing = doc.getIn(path);
  const stale = endpointMoved || (provider === 'openai' && existing === KEYLESS);
  if (existing && !stale) return existing;
  return provider === 'ollama' ? KEYLESS : `\${${variable}}`;
}
const llmKey = apiKey(['llm', 'apiKey'], 'TDAI_LLM_API_KEY', llmKeyGiven, llmMoved);
const embeddingKey = apiKey(['memory', 'embedding', 'apiKey'], 'TDAI_EMBEDDING_API_KEY', embeddingKeyGiven, embeddingMoved);
const embeddingKeyFromEnv = embeddingKey === '${TDAI_EMBEDDING_API_KEY}';
// Stop before writing anything: a stored key for another endpoint is cleared.
if (embeddingKeyFromEnv && !embeddingKeyGiven && (!embeddingKeyStored || embeddingMoved)) {
  console.error(`ERROR: GAH_GATEWAY_PROVIDER=${provider} at ${baseUrl} needs an embedding credential. Set GAH_GATEWAY_EMBEDDING_API_KEY, or use GAH_GATEWAY_PROVIDER=ollama for a local endpoint that needs no key.`);
  process.exit(1);
}

doc.setIn(['llm', 'baseUrl'], baseUrl);
doc.setIn(['llm', 'model'], llmModel || defaults.llmModel);
doc.setIn(['llm', 'apiKey'], llmKey);
doc.setIn(['memory', 'embedding', 'provider'], provider);
doc.setIn(['memory', 'embedding', 'baseUrl'], baseUrl);
doc.setIn(['memory', 'embedding', 'model'], model);
doc.setIn(['memory', 'embedding', 'dimensions'], dimensions);
// Ollama's OpenAI-compatible endpoint rejects the `dimensions` request field.
doc.setIn(['memory', 'embedding', 'sendDimensions'], provider !== 'ollama');
doc.setIn(['memory', 'embedding', 'apiKey'], embeddingKey);
writeFileSync(configPath, String(doc));

if (llmMoved) console.log('llm-endpoint-changed');
if (embeddingMoved) console.log('embedding-endpoint-changed');
