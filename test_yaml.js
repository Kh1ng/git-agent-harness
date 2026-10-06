import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

// Using npm prefix to install yaml just for test
const require = createRequire(resolve(process.cwd(), 'package.json'));
const yaml = require('yaml');

const text = `
llm:
  baseUrl: "https://api.openai.com/v1"
  model: "gpt-4o"
embedding:
  provider: "none"
  baseUrl: "https://api.openai.com/v1"
  model: "text-embedding-3-small"
`;
writeFileSync('test.yml', text);

const doc = yaml.parseDocument(readFileSync('test.yml', 'utf8'));
doc.setIn(['llm', 'baseUrl'], 'http://127.0.0.1:11434');
doc.setIn(['embedding', 'provider'], 'ollama');
writeFileSync('test.yml', String(doc));
console.log(readFileSync('test.yml', 'utf8'));
