// Minimal stand-in for Ollama's OpenAI-compatible API (/v1/embeddings,
// /v1/chat/completions), used by scripts/validate-gateway-provider.sh.
// It records every request (never credential values) so the validation can
// show that the gateway executed embedding calls against the configured backend.
//   node scripts/ollama-api-stub.mjs <port> <request-log> [dimensions]
import { createServer } from 'node:http';
import { appendFileSync } from 'node:fs';

const [port, log, dims] = process.argv.slice(2);
const DIMENSIONS = Number(dims || 768);

/** Deterministic unit vector derived from the text, so recall can score it. */
function vector(text) {
  const out = new Array(DIMENSIONS).fill(0);
  for (let i = 0; i < text.length; i++) out[(text.charCodeAt(i) * 31 + i) % DIMENSIONS] += 1;
  const norm = Math.hypot(...out) || 1;
  return out.map((v) => v / norm);
}

createServer((req, res) => {
  let body = '';
  req.on('data', (chunk) => { body += chunk; });
  req.on('end', () => {
    let json = {};
    try { json = JSON.parse(body || '{}'); } catch { /* recorded as an empty request */ }
    appendFileSync(log, JSON.stringify({
      method: req.method,
      path: req.url,
      model: json.model,
      authorization: req.headers.authorization ? 'present' : 'absent',
      inputs: Array.isArray(json.input) ? json.input.length : (json.input ? 1 : 0),
      dimensions: json.dimensions,
    }) + '\n');
    res.setHeader('Content-Type', 'application/json');
    if (req.method === 'POST' && req.url === '/v1/embeddings') {
      const inputs = Array.isArray(json.input) ? json.input : [json.input ?? ''];
      return res.end(JSON.stringify({
        object: 'list',
        model: json.model,
        data: inputs.map((text, index) => ({ object: 'embedding', index, embedding: vector(String(text)) })),
        usage: { prompt_tokens: 0, total_tokens: 0 },
      }));
    }
    if (req.method === 'POST' && req.url === '/v1/chat/completions') {
      return res.end(JSON.stringify({
        id: 'stub',
        object: 'chat.completion',
        created: 0,
        model: json.model,
        choices: [{ index: 0, finish_reason: 'stop', message: { role: 'assistant', content: '[]' } }],
        usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
      }));
    }
    res.statusCode = 404;
    res.end('{"error":"not found"}');
  });
}).listen(Number(port), '127.0.0.1');
