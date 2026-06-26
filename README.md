# git-agent-harness

Local-first CLI control plane for git agents.

This repo starts test-first. The initial MVP does not call GitHub, GitLab, paid models, OpenHands, or any external provider. It only reads local fixtures/artifacts and writes local artifacts.

## Bootstrap

```bash
bash scaffold.sh
cargo test
```

The initial `cargo test` run is expected to fail until `gah` is implemented.
