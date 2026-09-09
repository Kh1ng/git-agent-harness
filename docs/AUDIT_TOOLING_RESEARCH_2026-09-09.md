# Audit tooling research

Research for [#855](https://github.com/Kh1ng/git-agent-harness/issues/855), checked on 2026-09-09 against GAH commit `8ef7651a` and the official sources linked below. No scanners were installed or run. This note recommends priorities, not an implementation plan.

## Recommendation

Structured scanner results would improve coverage and reproducibility, but their additional finding rate remains unmeasured. Reuse existing GitHub findings first. Prefer `cargo-audit` and `npm audit` when the audit must describe an exact checkout. Consider `cargo-deny` when a project has an explicit license or source policy. Keep Semgrep CE optional until a small rule selection demonstrates useful findings beyond CodeQL. A reusable skill can guide interpretation, but it cannot guarantee scanner execution.

## What GAH has today

The [audit instruction](../src/dispatch/prompts.rs) asks the backend to fetch Dependabot alerts, inspect dead code and complexity, and file findings as issues. It does not run a deterministic preprocessing step. Its `gh api` example has neither pagination nor an explicit alert-state filter. The API defaults to 30 results per page, so this example can yield incomplete context. Failed requests must remain distinguishable from an empty successful response. [Dependabot REST reference](https://docs.github.com/en/rest/dependabot/alerts#list-dependabot-alerts-for-a-repository).

Read-only repository API checks found the following on 2026-09-09:

- GAH is public. CodeQL default setup is configured with the default query suite and weekly schedule. Its language list covers Actions, JavaScript/TypeScript, Python, Rust, and Swift. Recent analysis records included successful Rust and Swift results. Configuration alone does not prove complete analysis of every commit. [Default setup API](https://api.github.com/repos/Kh1ng/git-agent-harness/code-scanning/default-setup), [analysis records](https://api.github.com/repos/Kh1ng/git-agent-harness/code-scanning/analyses?per_page=5).
- The Dependabot alerts endpoint returned successfully. Automatic Dependabot security updates are disabled, which is separate from alert availability. Secret scanning and push protection are enabled. Non-provider patterns and validity checks are disabled. [Repository settings API](https://api.github.com/repos/Kh1ng/git-agent-harness), [Dependabot security updates](https://docs.github.com/en/code-security/concepts/supply-chain-security/dependabot-security-updates).
- No checked-in Semgrep, cargo-audit, cargo-deny, or dedicated dependency-audit workflow appeared in [the workflow directory](../.github/workflows). GitHub-managed CodeQL exists despite the absence of a CodeQL workflow file. CI uses `npm ci`, but install-time audit output is not structured audit-job input.

## Coverage and tradeoffs

| Source or tool | Useful signal | Limit and recommendation |
|---|---|---|
| Dependabot | Known vulnerable dependencies from GitHub's dependency graph. | Default-branch signal, not an exact local checkout. It depends on supported manifests and reviewed advisories. Retain it, paginate results, and preserve unavailable status. [Coverage](https://docs.github.com/en/code-security/concepts/supply-chain-security/dependabot-alerts). |
| Existing CodeQL | Source-level security and coding-error findings across GAH's languages. Results support SARIF. | Reuse existing analyses with their commit and language metadata before duplicating expensive extraction locally. The query libraries are open source, but the CLI has separate usage terms. Public GitHub repositories qualify; private-repository entitlement cannot be inferred from a Copilot student plan. [CLI and licensing](https://docs.github.com/en/code-security/concepts/code-scanning/codeql/codeql-cli), [query repository](https://github.com/github/codeql). |
| Semgrep CE | Local source-pattern and dataflow checks with JSON or SARIF output. | CE supports JavaScript/TypeScript, Rust, and Swift, but its documented analysis is limited to a single function. It does not supply the commercial product's cross-file analysis. Useful for selected project-specific rules, not a replacement for CodeQL. [CE support](https://docs.semgrep.dev/semgrep-ce-languages), [CE output](https://semgrep.dev/products/community-edition/). |
| cargo-audit | RustSec advisory checks against the selected Cargo.lock, including packages recorded in that lockfile. JSON output and advisory database controls are available. | A package-version advisory match is not proof that vulnerable code executes. Prefer this narrow option for dependency findings without a broader policy. GAH has separate root and desktop lockfiles. [RustSec tool](https://github.com/RustSec/rustsec/tree/main/cargo-audit), [CLI source](https://github.com/RustSec/rustsec/blob/main/cargo-audit/src/commands/audit.rs). |
| cargo-deny | Advisory, license, duplicate/banned-crate, and source checks over the Rust dependency graph. | Broader policy requires owner decisions. A duplicate version is not automatically a defect. JSON diagnostics are line-oriented; feature and target selection affect coverage. Prefer it over stacking two Rust advisory scanners when these policies become necessary. [Checks](https://embarkstudios.github.io/cargo-deny/checks/index.html), [CLI](https://embarkstudios.github.io/cargo-deny/cli/common.html). |
| npm audit | Known dependency vulnerabilities and dependency-chain remediation context, available as JSON. | Requires registry access and normally a lockfile. It sends package metadata to the configured registry; fallback requests include the full lockfile tree. It is not a license or source-code audit. Omitted dependency categories reduce coverage. Use the existing workspace lockfile, with scope recorded. [npm 10 reference](https://docs.npmjs.com/cli/v10/commands/npm-audit/). |

### Dependabot gaps need precise labels

Dependabot does not universally miss transitive packages. GitHub explicitly supports static transitive analysis for npm with `package-lock.json`. Its current table lists Cargo manifests and lockfiles, but marks static transitive support unsupported. The same column also describes direct/transitive relationship labeling. Therefore, a missing relationship label is not proof that GitHub missed a vulnerable package. Complete Cargo-transitive coverage must remain unverified until compared with local lockfile results. [Official ecosystem table](https://docs.github.com/en/code-security/reference/supply-chain-security/dependency-graph-supported-package-ecosystems), [table source with support markers](https://github.com/github/docs/blob/main/content/code-security/reference/supply-chain-security/dependency-graph-supported-package-ecosystems.md).

License-policy checks and secret discovery are separate from dependency advisory alerts. GitHub secret scanning already checks full Git history across branches for supported secret types. Its results are a separate API input, not part of Dependabot. The secret-scanning API can return raw secrets, so audit context must exclude credential values. [Secret scanning scope](https://docs.github.com/en/code-security/concepts/secret-security/secret-scanning), [API schema](https://docs.github.com/en/rest/secret-scanning/secret-scanning).

Gitleaks is an optional local history scanner for GitLab, self-hosted repositories, or rule gaps. It supports history selection, JSON/SARIF, and redaction. Its upstream now describes it as feature complete, with future releases limited to security patches. A shallow checkout cannot establish full-history coverage. Reassess maintenance and rule coverage before choosing it. [Gitleaks upstream](https://github.com/gitleaks/gitleaks).

### Operational costs and data boundaries

Semgrep's LGPL engine license does not imply unrestricted use of every rule pack. Semgrep-maintained rules use separate terms with internal-use, competing-product, and SaaS restrictions. Review the selected rules before distributing them through GAH. Local rules plus `--metrics off` avoid registry-triggered telemetry. [Licensing distinction](https://semgrep.dev/blog/2024/important-updates-to-semgrep-oss), [metrics controls](https://docs.semgrep.dev/metrics).

Read-only audit execution must exclude autofix, lockfile regeneration, and policy initialization. In particular, cargo-audit can generate a missing lockfile, and `npm audit fix` invokes installation. Missing inputs belong in the coverage report. Scanner cache/network activity also requires an explicit execution boundary. [cargo-audit source](https://github.com/RustSec/rustsec/blob/main/cargo-audit/src/commands/audit.rs), [npm behavior](https://docs.npmjs.com/cli/v10/commands/npm-audit/).

## Skill versus dispatch instruction

Recommendation: keep scanner execution, timeout handling, output limits, and provenance outside natural-language instructions. Deterministic preprocessing can distinguish findings from errors and attach the exact commit, tool/rule versions, lockfiles, and scan time. A skill should explain severity, reachability uncertainty, deduplication, and evidence needed before filing an issue. Raw tool findings remain untrusted evidence, not instructions to execute.

This distinction matches the current code. [Manager chat](../apps/server/src/managerChat/ManagerChatManager.ts) injects versioned bound skill text into prompts. That does not execute a scanner. [Rust dispatch](../src/dispatch/prompts.rs) builds a separate audit instruction and does not call that manager-chat binding path. The still-open [#830](https://github.com/Kh1ng/git-agent-harness/issues/830) concerns shared dispatch memory, not guaranteed scanner execution. A future integration need not wait for memory-gateway expansion.

Keep the audit instruction short: interpret supplied evidence, disclose gaps, validate candidate findings, and follow the job's reporting policy. Avoid duplicating invocation recipes across backend prompts and skills. Local scanners can serve GitHub and custom-domain GitLab profiles; the GitHub alerts API remains provider-specific.

## Unknowns before selecting a default

- Additional true findings, false positives, runtime, and disk costs were not benchmarked. No scanner output was produced during this research.
- Actual graph completeness, chosen Semgrep rule licenses, acceptable dependency licenses, and custom-registry audit support need project-specific evidence.
- Existing hosted findings can be stale or lack permissions on another profile. No-access, disabled, failed, partial, and clean are distinct outcomes.
- Dead-code and APOSD complexity review remain separate work. None of these tools establishes that code is unnecessary or that a design is simple.

The evidence supports reusing existing hosted results and adding narrow checkout-specific dependency evidence. It does not yet justify enabling every scanner for every audit.
