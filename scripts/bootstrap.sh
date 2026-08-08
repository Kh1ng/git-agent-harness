#!/usr/bin/env bash
# One-line bootstrap for a machine with nothing installed yet (Linux or
# macOS): installs missing prerequisites via their own official installers
# (rustup, nvm) rather than reimplementing package management, clones this
# repo, and hands off to scripts/install.sh.
#
# Usage -- env vars go on the `bash` side of the pipe, not the `curl` side,
# or they won't reach this script:
#   curl -fsSL https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh \
#     | GAH_GATEWAY_MODE=remote GAH_GATEWAY_URL=https://central.example.com:8420 \
#       GAH_GATEWAY_API_KEY=<key> bash
#
# GAH_INSTALL_DIR overrides the clone location (default: ~/git-agent-harness).
# Every GAH_*/GAH_SERVER_* env var scripts/install.sh understands is
# forwarded straight through -- this script doesn't interpret any of them.
set -euo pipefail

os="$(uname -s)"
case "$os" in
  Linux|Darwin) ;;
  *)
    echo "ERROR: unsupported OS '$os' -- this bootstrapper supports Linux and macOS only." >&2
    exit 1
    ;;
esac

if ! command -v git >/dev/null 2>&1; then
  echo "ERROR: git is required and not on PATH. Install it (e.g. 'xcode-select --install' on macOS, your distro's package manager on Linux) and re-run." >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "Installing Rust (rustup)..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

node_major=0
if command -v node >/dev/null 2>&1; then
  node_major="$(node -v | sed 's/^v//' | cut -d. -f1)"
fi
if [ "$node_major" -lt 20 ]; then
  echo "Installing Node.js (nvm)..."
  export NVM_DIR="$HOME/.nvm"
  if [ ! -s "$NVM_DIR/nvm.sh" ]; then
    curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
  fi
  # shellcheck disable=SC1091
  . "$NVM_DIR/nvm.sh"
  nvm install 22
  nvm use 22
fi

install_dir="${GAH_INSTALL_DIR:-$HOME/git-agent-harness}"
if [ -d "$install_dir/.git" ]; then
  echo "Using existing checkout at $install_dir"
  git -C "$install_dir" pull --ff-only
else
  echo "Cloning git-agent-harness into $install_dir..."
  # --depth 1: this is an install, not a dev checkout -- nobody bootstrapping
  # a new node needs full history on day one, and a shallow clone sidesteps
  # the repo's git history entirely (some early commits carry ~1.4GB of
  # accidentally-committed build artifacts that predate today's .gitignore
  # coverage -- rewriting that history is explicitly off the table, so this
  # is the size fix instead: 1.7MB shallow vs. ~350MB full clone).
  # `git fetch --unshallow` later recovers full history if ever wanted.
  git clone --depth 1 https://github.com/Kh1ng/git-agent-harness.git "$install_dir"
fi

cd "$install_dir"
# A fresh `cargo run -- update` here compiles the whole project -- expect
# this first run to take several minutes, not something to optimize away.
exec scripts/install.sh
