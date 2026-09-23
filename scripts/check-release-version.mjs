import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const jsonVersion = (file) => JSON.parse(readFileSync(join(root, file), 'utf8')).version;
const cargoVersion = (file) => readFileSync(join(root, file), 'utf8').match(/^version = "([^"]+)"/m)?.[1];
const versions = new Map([
  ['package.json', jsonVersion('package.json')],
  ['Cargo.toml', cargoVersion('Cargo.toml')],
  ['apps/server/package.json', jsonVersion('apps/server/package.json')],
  ['apps/web/package.json', jsonVersion('apps/web/package.json')],
  ['apps/mcp-server/package.json', jsonVersion('apps/mcp-server/package.json')],
  ['packages/contracts/package.json', jsonVersion('packages/contracts/package.json')],
  ['packages/contracts/src/coordinator-protocol.json', jsonVersion('packages/contracts/src/coordinator-protocol.json')],
  ['packages/shared/package.json', jsonVersion('packages/shared/package.json')],
  ['apps/desktop/package.json', jsonVersion('apps/desktop/package.json')],
  ['apps/desktop/Cargo.toml', cargoVersion('apps/desktop/Cargo.toml')],
  ['apps/desktop/tauri.conf.json', jsonVersion('apps/desktop/tauri.conf.json')],
]);
const expected = versions.get('package.json');
const mismatches = [...versions].filter(([, version]) => version !== expected);
if (mismatches.length) throw new Error(`Release versions differ: ${[...versions].map(([file, version]) => `${file}=${version}`).join(', ')}`);

const tag = process.argv[2];
if (tag?.startsWith('v') && tag !== `v${expected}`) throw new Error(`Tag ${tag} does not match product version ${expected}.`);
console.log(`Release version ${expected} is consistent${tag ? ` with ${tag}` : ''}.`);
