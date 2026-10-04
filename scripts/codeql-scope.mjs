import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const languages = [
  { language: 'actions', os: 'ubuntu-latest', build_mode: 'none' },
  { language: 'java-kotlin', os: 'ubuntu-latest', build_mode: 'none' },
  { language: 'javascript-typescript', os: 'ubuntu-latest', build_mode: 'none' },
  { language: 'python', os: 'ubuntu-latest', build_mode: 'none' },
  { language: 'rust', os: 'ubuntu-latest', build_mode: 'none' },
  { language: 'swift', os: 'macos-latest', build_mode: 'manual' },
];

/** Select whole-language scans from source and build inputs, including deleted paths. */
export function codeqlScope(paths, all = false) {
  const selected = new Set();
  for (const path of paths) {
    const name = path.split('/').at(-1);
    if (path === '.github/workflows/codeql.yml' || path.startsWith('.github/codeql/')
      || /^scripts\/codeql-scope(?:\.test)?\.mjs$/.test(path)) all = true;
    if (/^\.github\/(workflows|actions)\/.+\.ya?ml$/.test(path)) selected.add('actions');
    if (/\.(java|kt|kts)$/.test(name) || /^(pom\.xml|build\.gradle|settings\.gradle|gradle\.properties|gradlew(?:\.bat)?)$/.test(name)
      || /\/gradle\//.test(path)) selected.add('java-kotlin');
    if (/\.(?:[cm]?[jt]sx?|html)$/.test(name)
      || /^(package(?:-lock)?\.json|npm-shrinkwrap\.json|tsconfig.*\.json|\.npmrc|yarn\.lock|pnpm-lock\.yaml|bun\.lockb?)$/.test(name)) selected.add('javascript-typescript');
    if (/\.py$/.test(name) || /^(pyproject\.toml|requirements.*\.txt|setup\.cfg|Pipfile(?:\.lock)?|poetry\.lock|uv\.lock)$/.test(name)) selected.add('python');
    if (/\.rs$/.test(name) || /^(Cargo\.(toml|lock)|rust-toolchain(?:\.toml)?|rust-project\.json)$/.test(name)
      || /(^|\/)\.cargo\//.test(path)) selected.add('rust');
    if (/\.(swift|pbxproj|xcconfig|xcscheme)$/.test(name) || name === 'Package.resolved'
      || (path.startsWith('apps/ios/') && /\.(plist|entitlements)$/.test(name))) selected.add('swift');
  }
  return { include: languages.filter(row => all || selected.has(row.language)) };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.slice(2).some(arg => arg !== '--all')) throw new Error('Usage: codeql-scope.mjs [--all]');
  const matrix = codeqlScope(readFileSync(0, 'utf8').split('\0').filter(Boolean), process.argv.includes('--all'));
  console.log(`matrix=${JSON.stringify(matrix)}`);
  console.log(`has_languages=${matrix.include.length > 0}`);
}
