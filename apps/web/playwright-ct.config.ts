import { defineConfig } from '@playwright/experimental-ct-react';

export default defineConfig({
  testDir: './tests/component',
  use: { ctViteConfig: { define: { __GAH_VERSION__: JSON.stringify('test'), __GAH_COMMIT__: JSON.stringify('test') } } }
});
