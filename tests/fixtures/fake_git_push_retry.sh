#!/bin/sh
# Stand-in for `git` used by
# worktree::tests::push_retries_fake_git_timeout_once_then_completes.
#
# This file is checked into the repo (with the executable bit tracked by
# git) instead of being written and chmod'd at test run time. `cargo test`
# runs tests in parallel threads within one process, and on Linux a write-mode
# fd open on a file (even briefly, mid-`fs::write`) can be inherited by an
# unrelated thread's concurrent fork() and cause exec() of that file to fail
# with ETXTBSY ("Text file busy") until the fd is closed. An immutable,
# pre-existing script has no write phase, so that race cannot occur here.
#
# The counter file path is passed positionally (argv[3], i.e. the `push_url`
# slot in `git push -q <push_url> <branch>`) so this script never needs to be
# regenerated per test invocation.
count_file="$3"
count=0
[ -f "$count_file" ] && count=$(cat "$count_file")
count=$((count + 1))
printf '%s' "$count" > "$count_file"
if [ "$count" -eq 1 ]; then
  echo 'ssh: connect to host github.com port 22: Connection timed out' >&2
  exit 1
fi
exit 0
