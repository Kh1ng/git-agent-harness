# CodeQL scans

Pull requests and pushes to `main` scan only languages with changed source or build inputs.
Each selected scan analyzes the complete language database, including unchanged source files.
The weekly schedule and manual runs scan all six languages.

| Changed files | Selected scan | Runner |
| --- | --- | --- |
| Swift source, Xcode project files, iOS build configuration, Swift package lock | Swift | macOS |
| JavaScript, TypeScript, HTML, npm dependencies, TypeScript configuration | JavaScript/TypeScript | Linux |
| Rust source, Cargo manifests and locks, Rust toolchain configuration | Rust | Linux |
| Android Java source, Gradle inputs | Java/Kotlin | Linux |
| Python source and dependencies | Python | Linux |
| GitHub Actions workflow and action YAML | Actions | Linux |
| CodeQL workflow, configuration, or selector | All languages | Linux and macOS |
| Documentation or unrelated shell scripts | None | No analysis runner |

The selector uses the complete git diff, including deleted files and both paths of renamed files.
Pull requests use the merge base, so unrelated changes on `main` do not select extra languages.
New pull-request commits cancel the previous CodeQL workflow for that pull request.
The `CodeQL scans` check reports the combined result, including successful skips when no language needs analysis.

## Activate the replacement

The repository currently uses GitHub's default setup, which scans every enabled language on each pull request.
The replacement workflow waits for the repository variable `CODEQL_ADVANCED=true`.
This permits review before the cutover and prevents duplicate scans during that review.

1. Merge the pull request that adds `.github/workflows/codeql.yml`.
2. In repository Settings, open Advanced Security and switch CodeQL analysis from default setup to advanced setup.
3. Keep the committed workflow when GitHub offers a generated workflow.
4. Under Settings, open Secrets and variables, then Actions, then Variables.
5. Create the repository variable `CODEQL_ADVANCED` with the value `true`.
6. Run the CodeQL workflow manually from `main` to create the initial baseline for all six languages.
7. Check that all six analyses upload successfully before relying on the new pull-request checks.

Steps 2 and 5 form the cutover. Complete both together.
The variable enables the replacement workflow. The default setup must be inactive before its analyses upload.
Swift remains in the workflow and the weekly baseline.

To use the CLI for steps 2 and 5:

```bash
gh api --method PATCH repos/Kh1ng/git-agent-harness/code-scanning/default-setup \
  -f state=not-configured
gh variable set CODEQL_ADVANCED --repo Kh1ng/git-agent-harness --body true
gh workflow run codeql.yml --repo Kh1ng/git-agent-harness --ref main
```

If a baseline scan fails, restore default setup while you correct the replacement workflow.
To restore default setup, set `CODEQL_ADVANCED=false`, then enable default setup with all existing languages, including Swift.

GitHub documents the [switch to advanced setup](https://docs.github.com/en/code-security/how-tos/find-and-fix-code-vulnerabilities/configure-code-scanning/configuring-advanced-setup-for-code-scanning)
and the [supported build modes](https://docs.github.com/en/code-security/reference/code-scanning/codeql/build-options-for-compiled-languages).
Java uses the existing `none` build mode because the Android application currently contains Java source.
If Kotlin source is added, change that scan to a Kotlin-compatible build mode.

## Check the selector

```bash
node --test scripts/codeql-scope.test.mjs
printf '%s\0' apps/ios/GAH/GAHApp.swift | node scripts/codeql-scope.mjs
printf '%s\0' apps/server/src/quota.ts src/quota.rs | node scripts/codeql-scope.mjs
```
