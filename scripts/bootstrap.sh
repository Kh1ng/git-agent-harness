#!/usr/bin/env bash
# The paste-and-go install for Linux and macOS:
#
#   curl -fsSL https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh | bash
#
# While the repository is private, fetch it with a token and pass it along:
#
#   curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" \
#     https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh \
#     | GITHUB_TOKEN="$GITHUB_TOKEN" bash
#
# It needs only git and curl, clones this repository, downloads the published
# gah CLI, and hands the terminal to
# `gah setup`, which asks what this machine is for, checks and offers
# everything else, installs the service, and adds a first project.
#
# The commands GAH's own Settings page generates pass their choices as
# environment variables on the `bash` side of the pipe; they become
# `gah setup` flags below, so setup asks only what they left open:
#   GAH_NODE_ROLE=central|standalone|worker   GAH_CENTRAL_URL=<url>
#   GAH_GATEWAY_MODE=remote|colocated GAH_GATEWAY_URL=<url> GAH_GATEWAY_MEMORYCORE_PATH=<dir>
# Secrets (COORDINATOR_TOKEN, GAH_GATEWAY_API_KEY, GAH_GATEWAY_LLM_API_KEY) stay
# in the environment; setup reads them there and never takes them as flags.
#
# GAH_INSTALL_DIR overrides the clone location (default: ~/git-agent-harness).
# GAH_YES=1 accepts every default and offer without prompting.
# GAH_FROM_SOURCE=1 builds the CLI with Rust, offering to install rustup.
# GAH_VERSION selects a release tag (for example v0.2.0); default: latest.
set -euo pipefail

os="$(uname -s)"
case "$os" in
  Linux|Darwin) ;;
  *)
    if [ "${GAH_FROM_SOURCE:-}" = 1 ]; then
      echo "ERROR: this installer supports Linux and macOS. On Windows, use the GAH installer from the releases page." >&2
    else
      echo "ERROR: no published CLI for $os/$(uname -m). Run again with GAH_FROM_SOURCE=1." >&2
    fi
    exit 1
    ;;
esac

# curl owns stdin (this script arrives through it), so questions go to the terminal.
if [ "${GAH_YES:-}" != 1 ]; then
  if ! { true </dev/tty; } 2>/dev/null; then
    echo "ERROR: no terminal to ask questions on. Run this in a terminal, or set GAH_YES=1 to accept every default." >&2
    exit 1
  fi
fi

ask() {
  [ "${GAH_YES:-}" = 1 ] && return 0
  local answer
  printf '%s [Y/n] ' "$1" >/dev/tty
  read -r answer </dev/tty
  case "$answer" in ""|y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

if ! command -v git >/dev/null 2>&1; then
  if [ "$os" = Darwin ]; then
    echo "git is missing. Install Apple's command line tools with: xcode-select --install" >&2
  else
    echo "git is missing. Install it with your package manager (for example: sudo apt-get install -y git)." >&2
  fi
  echo "Then run this installer again." >&2
  exit 1
fi

if [ "${GAH_FROM_SOURCE:-}" = 1 ]; then
  if ! command -v cargo >/dev/null 2>&1; then
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    echo "GAH is built from source with Rust, which is not installed."
    if ! ask "Install Rust for your user with rustup (no administrator password)?"; then
      echo "Install Rust from https://rustup.rs, then run this installer again." >&2
      exit 1
    fi
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
fi

# While the repository is private, GITHUB_TOKEN (read access) authenticates
# the clone and later pulls. It reaches git through environment config, never
# argv or the saved remote URL.
if [ -n "${GITHUB_TOKEN:-}" ]; then
  basic="$(printf 'x-access-token:%s' "$GITHUB_TOKEN" | base64 | tr -d '\n')"
  export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.https://github.com/.extraheader
  export GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $basic"
fi

install_dir="${GAH_INSTALL_DIR:-$HOME/git-agent-harness}"
if [ -d "$install_dir/.git" ]; then
  echo "Updating the existing checkout at $install_dir"
  git -C "$install_dir" pull --ff-only
else
  echo "Cloning git-agent-harness into $install_dir"
  # A shallow clone: an install needs today's tree, not history (early
  # commits carry large, since-ignored build artifacts; 1.7MB vs ~350MB).
  git clone --depth 1 https://github.com/Kh1ng/git-agent-harness.git "$install_dir"
fi

cd "$install_dir"
if [ "${GAH_FROM_SOURCE:-}" = 1 ]; then
  echo "Building gah. The first build takes a few minutes."
  cargo build --locked --release --bin gah
  cli=target/release/gah
else
  arch="$(uname -m)"
  case "$os/$arch" in
    Linux/x86_64) asset=gah-linux-x86_64 ;;
    Darwin/*) asset=gah-macos-universal ;;
    *) echo "ERROR: no published CLI for $os/$arch. Run again with GAH_FROM_SOURCE=1." >&2; exit 1 ;;
  esac
  release_base=https://github.com/Kh1ng/git-agent-harness/releases
  if [ -n "${GAH_VERSION:-}" ]; then
    url="$release_base/download/$GAH_VERSION/$asset"
  else
    url="$release_base/latest/download/$asset"
  fi
  # Stage privately beside the destination so failed downloads cannot replace it.
  mkdir -p "$HOME/.local/bin"
  staging="$(mktemp -d "$HOME/.local/bin/.gah-download.XXXXXX")"
  trap 'rm -rf "$staging"' EXIT
  download_args=(-fsSL --output "$staging/gah")
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    printf 'Authorization: Bearer %s\n' "$GITHUB_TOKEN" >"$staging/header"
    download_args+=(--header "@$staging/header")
  fi
  if ! curl "${download_args[@]}" "$url"; then
    echo "ERROR: failed to download $url. Run again with GAH_FROM_SOURCE=1." >&2
    exit 1
  fi
  chmod +x "$staging/gah"
  # No checksum is published; running --version is the only verification.
  if ! reported_version="$("$staging/gah" --version)"; then
    echo "ERROR: downloaded CLI failed --version verification." >&2
    exit 1
  fi
  if [ -n "${GAH_VERSION:-}" ] && [ "$reported_version" != "gah ${GAH_VERSION#v}" ]; then
    echo "ERROR: downloaded CLI version does not match $GAH_VERSION: $reported_version" >&2
    exit 1
  fi
  cli="$HOME/.local/bin/gah"
  mv "$staging/gah" "$cli"
  rm -rf "$staging"
  trap - EXIT
fi

args=(setup --source "$install_dir")
case "${GAH_NODE_ROLE:-}" in
  central|standalone|worker) args+=(--role "$GAH_NODE_ROLE") ;;
  "") ;;
  *) echo "ERROR: GAH_NODE_ROLE must be central, standalone, or worker." >&2; exit 1 ;;
esac
[ -z "${GAH_CENTRAL_URL:-}" ] || args+=(--central-url "$GAH_CENTRAL_URL")
case "${GAH_GATEWAY_MODE:-}" in
  remote|colocated) args+=(--memory "$GAH_GATEWAY_MODE") ;;
  "") ;;
  *) echo "ERROR: GAH_GATEWAY_MODE must be remote or colocated." >&2; exit 1 ;;
esac
[ -z "${GAH_GATEWAY_URL:-}" ] || args+=(--gateway-url "$GAH_GATEWAY_URL")
[ -z "${GAH_GATEWAY_MEMORYCORE_PATH:-}" ] || args+=(--memorycore "$GAH_GATEWAY_MEMORYCORE_PATH")
[ "${GAH_YES:-}" != 1 ] || args+=(--yes)

if [ "${GAH_YES:-}" = 1 ]; then
  exec "$cli" "${args[@]}"
fi
exec "$cli" "${args[@]}" </dev/tty
