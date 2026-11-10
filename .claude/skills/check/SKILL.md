---
name: check
description: Run the same checks as CI (fmt, clippy, tests, docs, cargo deny, and the browser extension when it changed) and report what fails. Use before committing or opening a pull request, or when the user asks to check or verify the code.
---

# Check

Run what `.github/workflows/ci.yml` runs, locally, so a pull request doesn't fail on something that could have been seen here.

## Steps

1. See what changed against `main`:
   ```bash
   git fetch -q origin
   ./scripts/code-changes.sh origin/main...HEAD
   git status --short
   ```
   If the script prints nothing and the working tree has no changes outside docs, the change is docs-only: say so and stop. CI skips the build for it too.

2. Run the Rust checks in this order, and stop at the first that fails:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
   ```
   The first build of `velo-gui` compiles Slint and takes a few minutes.

3. Dependencies (licenses, advisories, sources; `deny.toml`):
   ```bash
   cargo deny check
   ```
   If `cargo-deny` isn't installed, say that this step was skipped and that CI will run it (`cargo install cargo-deny --locked` to install). A new advisory against a dependency fails this on every pull request, not only yours; the fix is a `cargo update -p <crate>` in its own pull request.

4. Only if something under `extension/` changed:
   ```bash
   cd extension && npm ci && npm run check && npm test && npm run build
   ```

## Report

- Say plainly which steps passed, which failed and which were skipped.
- For a failure, show the error output, not a summary of it.
- These checks ran on one OS. CI also builds on Linux, macOS (arm64 and Intel) and Windows (x64 and ARM64); say that those are not covered.
- Don't fix failures unless the user asked for that; `cargo fmt --all` is the exception.
