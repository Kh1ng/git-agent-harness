import { defineConfig } from '@playwright/experimental-ct-react';

export default defineConfig({
  testDir: './tests/component',
  outputDir: './test-results/component',
  use: { screenshot: 'only-on-failure', trace: 'retain-on-failure', ctViteConfig: { define: { __GAH_VERSION__: JSON.stringify('test'), __GAH_COMMIT__: JSON.stringify('test') } } }
});
