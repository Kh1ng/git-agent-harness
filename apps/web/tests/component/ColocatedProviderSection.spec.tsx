import { expect, test } from '@playwright/experimental-ct-react';
import React from 'react';
import { ColocatedProviderSection } from '../../src/pages/SettingsPage.js';

test('ColocatedProviderSection generates a setup command based on user inputs', async ({ mount, page }) => {
  const component = await mount(<ColocatedProviderSection />);

  const providerSelect = component.getByLabel('Provider', { exact: true });
  const copyButton = component.getByRole('button', { name: 'Copy' });

  // Initially only the provider dropdown should be visible, maybe memoryCorePath.
  await expect(providerSelect).toBeVisible();

  // Select Ollama
  await providerSelect.selectOption('ollama');

  const commandPre = component.locator('pre');
  await expect(commandPre).toBeVisible();

  const endpointInput = component.getByLabel('API Endpoint');
  const llmModelInput = component.getByLabel('LLM Model');
  const embeddingModelInput = component.getByLabel('Embedding Model');

  await endpointInput.fill('http://127.0.0.1:11434');
  await llmModelInput.fill('llama3');
  await embeddingModelInput.fill('nomic-embed-text');

  await expect(commandPre).toHaveText(
    "GAH_GATEWAY_MODE=colocated GAH_GATEWAY_MEMORYCORE_PATH='~/TencentDB-Agent-Memory/MemoryCore' GAH_GATEWAY_PROVIDER='ollama' GAH_GATEWAY_ENDPOINT='http://127.0.0.1:11434' GAH_GATEWAY_LLM_MODEL='llama3' GAH_GATEWAY_EMBEDDING_MODEL='nomic-embed-text' scripts/install.sh"
  );

  // Select OpenAI
  await providerSelect.selectOption('openai');
  await endpointInput.fill('https://api.openai.com/v1');
  await llmModelInput.fill('gpt-4o');
  await embeddingModelInput.fill('text-embedding-3-small');

  await expect(commandPre).toHaveText(
    "GAH_GATEWAY_MODE=colocated GAH_GATEWAY_MEMORYCORE_PATH='~/TencentDB-Agent-Memory/MemoryCore' GAH_GATEWAY_PROVIDER='openai' GAH_GATEWAY_ENDPOINT='https://api.openai.com/v1' GAH_GATEWAY_LLM_MODEL='gpt-4o' GAH_GATEWAY_EMBEDDING_MODEL='text-embedding-3-small' scripts/install.sh"
  );

  // Wait, the component might just auto-update the command textbox directly without a "Reveal" button.
  // Let's test the Copy behavior.
  let clipboardText = '';
  await page.exposeFunction('setClipboardText', (text: string) => {
    clipboardText = text;
  });
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async (t: string) => window['setClipboardText'](t) }
    });
  });

  await copyButton.click();
  // Playwright won't assert clipboard natively if we mock it, but we can verify our exposed function.
  expect(clipboardText).toContain("GAH_GATEWAY_PROVIDER='openai'");
});
