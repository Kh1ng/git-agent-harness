### Provider Configuration and Validation (issue #1319)

To support local embedding with Ollama, the installer parses `GAH_GATEWAY_PROVIDER` (either `openai` or `ollama`) and mutates `tdai-gateway.local.yaml` to configure the chosen backend, endpoint, and models for both LLM and embedding.

When `GAH_GATEWAY_PROVIDER=ollama`:
- `llm.baseUrl` and `embedding.baseUrl` point to the Ollama endpoint (e.g. `http://127.0.0.1:11434`)
- `llm.model` is set to the selected LLM (e.g. `llama3`)
- `embedding.provider` is set to `ollama` and `embedding.model` is set to the selected embedding model (e.g. `nomic-embed-text`)

Limitations:
- Provider validation requires the Kh1ng fork of TencentDB-Agent-Memory to parse the `ollama` provider for embedding.

Validation Evidence:
Supported gateway contract reference: `kh1ng/TencentDB-Agent-Memory` at commit `v0.2.1-fork.1` (or any branch including the `ollama` embedding provider implementation).

Sanitized configuration (`tdai-gateway.local.yaml`):
```yaml
llm:
  baseUrl: http://127.0.0.1:11434
  model: llama3
embedding:
  provider: ollama
  baseUrl: http://127.0.0.1:11434
  model: nomic-embed-text
```

Verification command:
```bash
curl -f http://127.0.0.1:8420/health
```

Result:
```json
{"status":"ok","llm":"healthy","embedding":"healthy","store":"connected"}
```
