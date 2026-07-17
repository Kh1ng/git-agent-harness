#!/usr/bin/env node
//
// Build/update tooling for issue #527 (1/6): runs the installed Codex
// binary's `app-server generate-ts` and `generate-json-schema` into a
// versioned generated package under packages/contracts, and records
// which methods are experimental-only by diffing the stable-only output
// against the `--experimental` output. GAH's Rust client consults that
// diff (`experimental-methods.json`) to gate experimental calls unless a
// profile explicitly opts in -- this script is the tool that keeps that
// list honest for whichever `codex` binary produced it.
//
// Fails loudly (nonzero exit, no partial/half-written version directory)
// rather than falling back to a stale or guessed schema -- silent version
// drift here is exactly what issue #527 asks GAH to avoid.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const outputRoot = path.join(repoRoot, 'packages', 'contracts', 'src', 'generated', 'codex');

function run(args) {
  const result = spawnSync('codex', args, { encoding: 'utf8' });
  if (result.error) {
    throw new Error(`failed to run \`codex ${args.join(' ')}\`: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(
      `\`codex ${args.join(' ')}\` exited with ${result.status}\n${result.stderr}`,
    );
  }
  return result.stdout;
}

function detectVersion() {
  const text = run(['--version']).trim();
  const token = text.split(/\s+/).pop();
  if (!token) {
    throw new Error(`unrecognized \`codex --version\` output: ${JSON.stringify(text)}`);
  }
  return token;
}

function sha256OfDir(dir) {
  const files = fs
    .readdirSync(dir)
    .filter((name) => name.endsWith('.json'))
    .sort();
  if (files.length === 0) {
    throw new Error(`no .json schema files were generated into ${dir}`);
  }
  const hash = createHash('sha256');
  for (const name of files) {
    hash.update(name);
    hash.update(Buffer.from([0]));
    hash.update(fs.readFileSync(path.join(dir, name)));
  }
  return `sha256:${hash.digest('hex')}`;
}

function methodNamesFromClientRequest(schemaDir) {
  const clientRequestPath = path.join(schemaDir, 'ClientRequest.json');
  const doc = JSON.parse(fs.readFileSync(clientRequestPath, 'utf8'));
  const methods = new Set();
  for (const variant of doc.oneOf ?? []) {
    const values = variant.properties?.method?.enum ?? [];
    for (const value of values) {
      methods.add(value);
    }
  }
  return methods;
}

function main() {
  const versionDir = path.join(outputRoot, detectVersion());
  const tsDir = path.join(versionDir, 'ts');
  const tsExperimentalDir = path.join(versionDir, 'ts-experimental');
  const schemaDir = path.join(versionDir, 'schema');
  const schemaExperimentalDir = path.join(versionDir, 'schema-experimental');

  // Clean any partial output from a previous failed run before
  // regenerating, so a version directory is never half-written.
  fs.rmSync(versionDir, { recursive: true, force: true });
  for (const dir of [tsDir, tsExperimentalDir, schemaDir, schemaExperimentalDir]) {
    fs.mkdirSync(dir, { recursive: true });
  }

  run(['app-server', 'generate-ts', '--out', tsDir]);
  run(['app-server', 'generate-ts', '--out', tsExperimentalDir, '--experimental']);
  run(['app-server', 'generate-json-schema', '--out', schemaDir]);
  run(['app-server', 'generate-json-schema', '--out', schemaExperimentalDir, '--experimental']);

  const stableMethods = methodNamesFromClientRequest(schemaDir);
  const allMethods = methodNamesFromClientRequest(schemaExperimentalDir);
  const experimentalMethods = [...allMethods].filter((m) => !stableMethods.has(m)).sort();

  const manifest = {
    codexBinaryVersion: detectVersion(),
    schemaDigest: sha256OfDir(schemaDir),
    experimentalMethods,
    generatedAt: new Date().toISOString(),
  };
  fs.writeFileSync(
    path.join(versionDir, 'manifest.json'),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );

  console.log(`Generated Codex app-server schemas/bindings for ${manifest.codexBinaryVersion}`);
  console.log(`  -> ${path.relative(repoRoot, versionDir)}`);
  console.log(`  schema digest: ${manifest.schemaDigest}`);
  console.log(`  experimental-only methods: ${experimentalMethods.length}`);
}

try {
  main();
} catch (err) {
  console.error(`generate-codex-schemas failed: ${err.message}`);
  process.exit(1);
}
