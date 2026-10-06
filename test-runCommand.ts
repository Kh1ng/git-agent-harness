import { execFileSync } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { unixSetupCommand } from './apps/server/src/nodeSetup.ts';

function runCommand(command: string, input = ''): string {
  const checkout = mkdtempSync(join(tmpdir(), 'gah-colocated-'));
  writeFileSync(join(checkout, 'curl'), `#!/bin/sh
cat <<'INNEREOF'
#!/bin/sh
printf 'path=%s\\n' "$GAH_GATEWAY_MEMORYCORE_PATH"
printf 'llm=%s\\n' "\${GAH_GATEWAY_LLM_API_KEY-<unset>}"
printf 'embedding=%s\\n' "\${GAH_GATEWAY_EMBEDDING_API_KEY-<unset>}"
INNEREOF
`);
  chmodSync(join(checkout, 'curl'), 0o755);
  return execFileSync('bash', ['-c', command.replaceAll('</dev/tty', '')], {
    cwd: checkout, input, encoding: 'utf8', env: { PATH: checkout + ':' + (process.env.PATH ?? ''), HOME: '/destination/home' },
  }).trimStart();
}

const ollamaCommand = unixSetupCommand('linux', 'central', 'http://127.0.0.1:3773', undefined, 'ollama', 'http://127.0.0.1:11434/v1', 'llama3', 'nomic-embed-text', undefined, undefined);
console.log("ollamaCommand: ", runCommand(ollamaCommand));

const openAiCommand = unixSetupCommand('linux', 'central', 'http://127.0.0.1:3773', undefined, 'openai', 'https://api.openai.com/v1', 'gpt-4o', 'text-embedding-3-small', undefined, undefined);
console.log("openAiCommand: ", runCommand(openAiCommand, 'generation-canary\nembedding-canary\n'));

