# Contributing to LX AudioLabs

Bug reports welcome, occasional pull requests too. Keep it simple.

## Reporting Bugs

Use the **Bug Report** issue template. Include:

- Plugin name and version
- Host/DAW you tested in
- Steps to reproduce
- What you expected vs. what happened

If you can, attach a minimal CLAP host log or audio sample.

## Pull Requests

1. Open an issue first — discuss before coding.
2. Keep PRs focused. One thing per PR.
3. Code must pass CI:
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo fmt --all -- --check`
   - `cargo test --workspace --lib`
4. In the PR description, mention which host(s) you tested in. "No host-facing changes" is valid when true.
5. Run `gitleaks detect --source .` before pushing — no secrets in commits.

### Contributor License Grant

This project is licensed under **GNU General Public License v3.0 or later** ([LICENSE](LICENSE)).

By opening a pull request, you agree:

1. You wrote the contribution or have the legal right to submit it.
2. The contribution is licensed under GPL-3.0-or-later.
3. You retain copyright; this is a license grant, not a transfer.

## Code Style

- Comments explain **why**, not what.
- Follow existing patterns in the codebase.
- `lx-dsp`, `lx-ui` changes need more discussion — open an issue first.
