# Contributing to lazycmake

Thanks for helping improve lazycmake.

## Development setup

```bash
git clone https://github.com/<you>/lazycmake.git
cd lazycmake
cargo test
cargo run -p lazycmake-tui -- -C /path/to/a/cmake/project
```

## Guidelines

- Prefer small, focused pull requests.
- Match existing Rust style in `lazycmake-core` / `lazycmake-tui`.
- Add or update unit tests in the same change when fixing behavior (especially parsers and config merges).
- Do not introduce project-specific toolchains (Zephyr, west, vendor SDKs) into the core — expose them via generic config (`env`, `env_file`, overrides) instead.
- Keep the README accurate when you change user-visible behavior or config keys.
- Keep `Cargo.lock` committed so CI `--locked` builds stay reproducible.
- Release binaries are produced by tagging `v*` (see README → Install → Binary releases).

## Project layout

| Crate | Role |
|-------|------|
| `lazycmake-core` | Presets, File API, ctest, config, env files — unit-testable without a TTY |
| `lazycmake-tui` | Ratatui UI and the `lazycmake` binary |

## Code of conduct

Be respectful. This is a small open-source tool; assume good intent and keep discussion technical.
