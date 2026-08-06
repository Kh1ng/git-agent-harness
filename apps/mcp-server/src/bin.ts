#!/usr/bin/env node
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { createGahMcpServer } from './server.js';

async function main() {
  const server = createGahMcpServer();
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error('gah-mcp-server listening on stdio');
}

main().catch((error) => {
  console.error('gah-mcp-server failed to start:', error);
  process.exit(1);
});
