# TICKET-116: Dispatch targets should come from GitHub/GitLab Issues, not docs/tickets/*.md

**Priority:** P1
**Profile:** gah

## Background

Every ticket dispatched this session (`--target docs/tickets/TICKET-XXX.md`) works by
committing a markdown file into the repo and pointing `--target` at its path. Look at
`build_task` in `src/dispatch.rs` (~line 2363/2434): the `target` string is embedded
*literally* as the "Focus" section:

```rust
if !target.is_empty() {
    task.push_str(&format!("\n## Focus\n\n{}\n", target));
}
```

When `--target` is a file path, "Focus" ends up being just that path string — the
dispatched agent has to independently open the file inside its worktree to read the
actual ticket content. This only works because the markdown files happen to be checked
into the repo and thus present in every worktree. It's a hand-rolled, repo-coupled
ticket system duplicating what GitHub Issues / GitLab Issues already do for free —
and GAH already shells out to `gh`/`glab` elsewhere for MR/PR work (`src/provider.rs`,
`src/sync.rs`, `src/dispatch.rs:1900` `ensure_bin("glab")`), and already has a
provider→CLI mapping: `Profile::provider_cli()` in `src/config.rs:424`
(`"github" => "gh"`, `"gitlab" => "glab"`).

## Task

1. Let `--target` accept an issue reference (e.g. `--target 42`, or `--target #42`) in
   addition to a file path (keep file-path support for local/one-off use — don't force
   a migration of every existing artifact).
2. When `--target` parses as an issue number, resolve it via the profile's
   `provider_cli()`:
   - github: `gh issue view <n> --repo <profile.repo> --json title,body`
   - gitlab: `glab issue view <n> --repo <profile.repo> -F json` (check actual flag —
     mirror whatever `src/provider.rs`/`src/sync.rs` already use for MR fetches, don't
     invent new gh/glab invocation conventions)
3. Feed the fetched title+body into `build_task`'s Focus section as literal text
   (matching today's behavior for `--target "some ticket text"` inline-string tests
   already in `src/dispatch.rs` ~line 3514) instead of a path.
4. `parse_ticket_metadata` (used for recommended-backend/priority routing) needs an
   issue-shaped input path too — check what it currently parses out of the markdown
   frontmatter/headers and map the equivalent out of the issue body/labels.
5. Migrate the existing `docs/tickets/TICKET-*.md` backlog into real GitHub issues
   (one issue per ticket, same numbering scheme in the title e.g. "TICKET-117: ...")
   so the two systems don't fork. Do not delete `docs/tickets/` until this dispatch
   flow is proven working end-to-end against a real issue.

## Acceptance Criteria

- [ ] `gah dispatch --target 42` (an issue number) works end-to-end against a real
      GitHub issue on this repo
- [ ] File-path `--target` still works unchanged (backward compatible)
- [ ] Existing open `docs/tickets/*.md` backlog has matching GitHub issues
- [ ] `cargo test` green, new tests cover the issue-number parse path

## Do NOT

- Do not build a GitLab-specific and GitHub-specific code path that diverges in
  behavior — route both through `provider_cli()` like the rest of the codebase does.
- Do not remove file-path `--target` support.
