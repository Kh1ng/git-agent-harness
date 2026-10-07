import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { execFileSync } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ColocatedProviderSection } from '../../src/pages/SettingsPage.js';
import { unixSetupCommand } from '../../../server/src/nodeSetup.js';

/** Run a generated command against a stub installer that reports what it receives. */
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
  // Prompts read stdin here so the test can answer them without a terminal.
  // Each hidden prompt prints a newline; only the installer's report matters.
  return execFileSync('bash', ['-c', command.replaceAll('</dev/tty', '')], {
    cwd: checkout, input, encoding: 'utf8', env: { PATH: checkout + ':' + (process.env.PATH ?? ''), HOME: '/destination/home' },
  }).trimStart();
}

test('ColocatedProviderSection generates a setup command based on user inputs', async ({ mount, page }) => {
  await page.route('/api/settings/nodes/command', async (route) => {
    const data = route.request().postDataJSON();
    const command = unixSetupCommand('linux', 'central', 'http://127.0.0.1:3773', data.gatewayUrl, data.provider, data.providerEndpoint, data.llmModel, data.embeddingModel, data.memoryCorePath, data.embeddingDimensions);
    await route.fulfill({ json: { command } });
  });

  const component = await mount(<ColocatedProviderSection />);

  const providerSelect = component.getByRole('combobox', { name: 'Provider' });
  const copyButton = component.getByRole('button', { name: 'Copy' });

  await expect(providerSelect).toBeVisible();

  // Ollama needs no credentials, so its command has no prompts.
  await providerSelect.selectOption('ollama');

  const endpointInput = component.getByLabel('API Endpoint');
  const llmModelInput = component.getByLabel('LLM Model');
  const embeddingModelInput = component.getByLabel('Embedding Model');

  await endpointInput.fill('http://127.0.0.1:11434/v1');
  await llmModelInput.fill('llama3');
  await embeddingModelInput.fill('nomic-embed-text');

  await component.getByRole('button', { name: 'Reveal setup command' }).click();

  const commandPre = component.locator('pre');
  await expect(commandPre).toBeVisible();

  // The default path's \`~\` stays unquoted so the node's shell expands it.
  const ollamaCommand = unixSetupCommand('linux', 'central', 'http://127.0.0.1:3773', undefined, 'ollama', 'http://127.0.0.1:11434/v1', 'llama3', 'nomic-embed-text', undefined, undefined);
  await expect(commandPre).toHaveText(ollamaCommand);
  expect(ollamaCommand).not.toContain('read -rsp');
  expect(runCommand(ollamaCommand)).toBe('path=/destination/home/TencentDB-Agent-Memory/MemoryCore\nllm=<unset>\nembedding=<unset>\n');

  // OpenAI prompts privately for an optional generation key and the required embedding key.
  await providerSelect.selectOption('openai');
  await endpointInput.fill('https://api.openai.com/v1');
  await llmModelInput.fill('gpt-4o');
  await embeddingModelInput.fill('text-embedding-3-small');

  await component.getByRole('button', { name: 'Reveal setup command' }).click();

  const openAiCommand = unixSetupCommand('linux', 'central', 'http://127.0.0.1:3773', undefined, 'openai', 'https://api.openai.com/v1', 'gpt-4o', 'text-embedding-3-small', undefined, undefined);
  await expect(commandPre).toHaveText(openAiCommand);
  expect(runCommand(openAiCommand, 'generation-canary\nembedding-canary\n')).toBe('path=/destination/home/TencentDB-Agent-Memory/MemoryCore\nllm=generation-canary\nembedding=embedding-canary\n');
  expect(runCommand(openAiCommand, '\nembedding-canary\n')).toBe('path=/destination/home/TencentDB-Agent-Memory/MemoryCore\nllm=<unset>\nembedding=embedding-canary\n');
  // Without an embedding key the command fails before the installer runs.
  expect(() => runCommand(openAiCommand, 'generation-canary\n\n')).toThrow();

  // Absolute paths stay literal; switching back to Ollama drops the prompts.
  await component.getByLabel('MemoryCore Path').fill("/srv/it's here");
  await providerSelect.selectOption('ollama');
  await component.getByRole('button', { name: 'Reveal setup command' }).click();
  await expect(commandPre).not.toContainText('read -rsp');
  expect(runCommand((await commandPre.textContent()) ?? '')).toContain("path=/srv/it's here\n");

  let clipboardText = '';
  await page.exposeFunction('setClipboardText', (text: string) => {
    clipboardText = text;
  });
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async (t: string) => (window as unknown as { setClipboardText: (text: string) => Promise<void> }).setClipboardText(t) }
    });
  });

  await copyButton.click();
  await expect.poll(() => clipboardText).toContain("GAH_GATEWAY_PROVIDER='ollama'");
});
