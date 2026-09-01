# lazycmake

**A terminal UI for CMake projects** — configure, build, run, and test without memorizing `cmake` / `ctest` invocations.

Inspired by [lazygit](https://github.com/jesseduffield/lazygit): three columns (Presets · Targets · Tests), a live output pane, and keyboard-driven workflows. Works with any CMake project that uses [CMake Presets](https://cmake.org/cmake/help/latest/manual/cmake-presets.7.html) (and degrades gracefully when features need a newer CMake).

lazycmake always invokes `cmake` and `ctest` directly. It does **not** wrap other meta-build tools.

![Status](https://img.shields.io/badge/status-early-yellow)
![License](https://img.shields.io/badge/license-MIT-blue)
![Rust](https://img.shields.io/badge/rust-2021-orange)

---

## Features

- **Presets column** — lists visible configure presets from `CMakePresets.json` / `CMakeUserPresets.json` (inherits, hidden, `$env{}` / `${sourceDir}` expansion)
- **Targets column** — discovered via the CMake File API (`codemodel-v2`) after configure; includes a synthetic `all` target
- **Tests column** — discovered with `ctest --show-only=json-v1` (or `ctest -N`); inline pass / fail / skip status after runs
- **Configure / build / run / test** — `c` `b` `r` `t`/`T`, with clean variants (`C` / `B`) and confirmation prompts
- **Live output** — streamed into a scrollable pane; fullscreen mode with vim-style `g` / `G`
- **Per-preset overrides** — non-standard configure (custom `-S`, cache vars) and optional per-preset environment (`env` / `env_file`)
- **Curated testing** — pin the Tests column to specific configure presets (e.g. a dedicated `tests` binary dir) independent of the selected build preset
- **Parallel by default** — builds and ctest runs use `--parallel <ncores>`

---

## Requirements

| Tool | Notes |
|------|--------|
| **Rust** | 1.70+ recommended (`cargo` on `PATH`) — only needed to build from source |
| **CMake** | 3.14+ useful; **3.19+** for presets; **3.23+** for preset `include` |
| **Ninja** (or another generator) | Whatever your presets declare |
| **ctest** | Bundled with CMake |
| **bash** | Only if you use `env_file` in config (script is sourced) |

Supported platforms: Linux, macOS, and Windows (via a terminal that works with [crossterm](https://github.com/crossterm-rs/crossterm) / WSL).

---

## Install

### Prebuilt binaries (recommended)

1. Open the latest [GitHub Release](https://github.com/<you>/lazycmake/releases).
2. Download the archive for your OS/CPU:

   | Platform | Archive name contains |
   |----------|------------------------|
   | Linux x86_64 | `x86_64-unknown-linux-gnu` |
   | Linux aarch64 | `aarch64-unknown-linux-gnu` |
   | macOS Apple Silicon | `aarch64-apple-darwin` |
   | macOS Intel | `x86_64-apple-darwin` |
   | Windows x86_64 | `x86_64-pc-windows-msvc` |

3. Verify the checksum (optional):

   ```bash
   shasum -a 256 -c lazycmake-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
   ```

4. Extract and put `lazycmake` on your `PATH`:

   ```bash
   tar -xzf lazycmake-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
   sudo install -m 755 lazycmake-v0.1.0-x86_64-unknown-linux-gnu/lazycmake /usr/local/bin/
   ```

   On Windows, unzip and move `lazycmake.exe` somewhere on your `PATH`.

### From source

```bash
git clone https://github.com/<you>/lazycmake.git
cd lazycmake
cargo install --path crates/lazycmake-tui --locked
```

Or run without installing:

```bash
cargo run -p lazycmake-tui --locked -- -C /path/to/your/cmake/project
```

---

## Quick start

```bash
# From your CMake project root (directory with CMakePresets.json or CMakeLists.txt)
lazycmake

# Or point at a project
lazycmake -C ~/src/my-project
lazycmake --project ~/src/my-project

# Optional: explicit config file or directory
lazycmake -C ~/src/my-project --config ~/src/my-project/.lazycmake
```

1. Select a **preset** → `Enter` or `c` to configure  
2. Select a **target** → `b` to build, `r` to run (executables)  
3. Select a **test** → `t` to build/run that test; `T` to build all then run the full suite  

Press `?` at any time for the in-app help overlay.

---

## User interface

```
┌─ Presets ────────┬─ Targets ────────────────┬─ Tests ──────────────────────┐
│> tests           │> all               [oth] │> FooTest                  ✓  │
│  debug           │  my_app            [exe] │  BarTest                  ✗  │
│  release         │  my_lib            [lib] │  BazTest                  -  │
│[2/12]            │[3/40]                    │[1/200]  ✓ 1  ✗ 1  - 198     │
└──────────────────┴──────────────────────────┴──────────────────────────────┘
┌─ Output ───────────────────────────────────────────────────────────────────┐
│ $ cmake --preset tests                                                     │
│ $ cmake --build build-test --parallel 16                                   │
└────────────────────────────────────────────────────────────────────────────┘
 Preset: tests   [↑↓] Move  [Enter] …  [o] Output  [?] Help  [q] Quit
```

| Column | Contents |
|--------|----------|
| **Presets** | Configure presets (alphabetical). Overrides can reveal otherwise-hidden presets. |
| **Targets** | File API targets after configure. Kinds: `exe` / `lib` / `utl` / `oth`. |
| **Tests** | CTest cases for the active testing preset / binary dir. |

Status glyphs: `✓` pass · `✗` fail · `◌` skip · `-` not run (ASCII fallbacks `+` / `x` / `o` / `-` when needed).

---

## Keyboard reference

### Navigation

| Key | Action |
|-----|--------|
| `↑` / `↓` or `j` / `k` | Move selection |
| `Tab` / `Shift+Tab` | Cycle Presets → Targets → Tests |
| `PgUp` / `PgDn` | Page lists |
| `Home` / `End` | Top / bottom of list |
| `/` | Fuzzy filter focused column (`Esc` clears) |
| `F` | Toggle failing/skipped-only (Tests column) |

### Actions

| Key | Action |
|-----|--------|
| `Enter` | Context action: configure / build / run selected test |
| `c` | Configure selected preset |
| `C` | Delete `CMakeCache.txt` + `CMakeFiles/`, then reconfigure (confirm) |
| `b` | Build selected target (`all` = default Ninja target) |
| `B` | Clean then build (confirm) |
| `r` | Run selected executable |
| `t` | Build/run selected test (`cmake --build … --target <name>_run`) |
| `T` | Build **all** test binaries, then `ctest` (same idea as `cmake --build && ctest`) |
| `o` | Fullscreen output |
| `?` | Help |
| `q` | Quit |
| `Esc` | Close help / cancel confirm / leave filter |

### Fullscreen output (`o`)

| Key | Action |
|-----|--------|
| `j` / `k` or arrows | Scroll |
| `PgUp` / `PgDn` | Page |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom (enables follow) |
| `f` | Toggle follow (tail) |
| `o` / `Esc` | Back — **re-enables follow** on the main output pane |

---

## Configuration

Config is optional. Without it, lazycmake still lists presets and works for ordinary `cmake --preset` projects.

### Resolution order

1. `~/.config/lazycmake/config.toml` (global defaults)
2. `--config PATH` (file **or** directory containing `config.toml`)
3. `<project>/.zed/.lazycmake/config.toml` (personal / editor-local)
4. `<project>/.lazycmake/config.toml` (project-local)

Later files **merge over** earlier ones (maps are unioned; scalars replaced).

Runtime UI state is stored in `<project>/.lazycmake/state.json` (last preset, scroll positions, etc.). Add that path to `.gitignore`.

### Example `config.toml`

See also [`examples/config.toml`](examples/config.toml).

```toml
[general]
# Selected on first launch if state.json has no last preset
default_preset = "tests"

# --- Tests column ----------------------------------------------------------
# When set, the Tests column always uses these configure preset(s), even if
# another preset is selected in the Presets column.
[testing]
curated_presets = ["tests"]

[testing.presets.tests]
# Directory passed to ctest (relative to project root), if different from binaryDir
test_dir = "build-test/src/tests"
# Appended to ctest (lazycmake also ensures --parallel <ncores>)
extra_args = ["--output-on-failure", "--parallel"]

# --- Non-standard configure ------------------------------------------------
# If source_dir / generator / cache_variables are set, lazycmake runs:
#   cmake -S <source_dir> -B <binaryDir> [-G …] -D…
# instead of: cmake --preset <name>
#
# Having an override entry also un-hides a preset marked "hidden" in JSON.
[presets.overrides.CustomBoard]
# Optional: source a shell script before configure/build/run for THIS preset only
env_file = "$env{TOOLCHAIN_ENV_FILE}"
# Optional: extra env vars (merged after env_file; values expand macros)
env = { BOARD_ROOT = "${sourceDir}/boards" }
source_dir = "$env{SDK_ROOT}/share/sysbuild"
cache_variables = { APP_DIR = "${sourceDir}", BOARD = "my_board" }

[ui]
theme = "dark"   # reserved for future theming
```

### Macros in override strings

| Macro | Expands to |
|-------|------------|
| `${sourceDir}` | Project root (directory with `CMakePresets.json`) |
| `${presetName}` | Preset name |
| `$env{VAR}` | Process environment variable `VAR` (error if unset) |
| `$penv{VAR}` | Same as `$env{VAR}` (CMake preset compatibility) |

### `env_file` and `env` (per preset)

Use these when a preset needs toolchain variables (for example an SDK root or an extended `PATH`) **without** polluting other presets:

1. If `env_file` is set, its path is expanded, then the file is `source`d in bash and exported variables are collected.
2. `env` entries are expanded and merged on top (they win on conflicts).
3. That map is applied while expanding `source_dir` / `cache_variables`, and injected into the configure / build / run child processes for that preset.

Host tests (`t` / `T`) do **not** load preset override env — they use the testing binary dir only.

### Testing without `testPresets`

Many projects use a normal configure preset (e.g. `tests`) with its own `binaryDir` instead of CMake `testPresets`. Resolution:

1. If `[testing].curated_presets` is set → Tests column follows that list (and optional `test_dir` / `extra_args`).
2. Else → Tests follow the **selected** configure preset’s binary dir.

`T` (test all) always:

1. `cmake --build <testing-binary-dir> --parallel <n>`
2. `ctest` in the resolved test directory (with your `extra_args` + parallel)

---

## CLI

```
lazycmake [OPTIONS]

Options:
  -C, --project <PATH>   Project directory (default: current working directory)
      --config <PATH>    config.toml file, or directory containing it
  -h, --help             Print help
```

The project path must contain `CMakePresets.json` or `CMakeLists.txt`.

---

## How jobs map to CMake

| Action | Command (simplified) |
|--------|----------------------|
| Configure | `cmake --preset <name>` **or** manual `-S/-B/-G/-D…` from override |
| Clean configure | Delete `CMakeCache.txt` + `CMakeFiles/`, then configure |
| Build | `cmake --build <binaryDir> [--target …] --parallel <n>` |
| Clean build | Same with an extra `--target clean` first |
| Run | Execute the selected target’s artifact path |
| Test one | `cmake --build <testBin> --target <CaseName>_run --parallel <n>` |
| Test all | Build all in test bin dir, then `ctest` with parallel |

Output is captured off-TTY so child tools cannot corrupt the alternate screen.

---

## CMake version capabilities

Detected once at startup:

| Feature | Minimum | If older |
|---------|---------|----------|
| Presets | 3.19 | Presets column empty |
| Preset `include` | 3.23 | Includes skipped (warn) |
| File API codemodel | 3.14 | Targets column empty |
| `ctest --show-only=json-v1` | 3.14 | Falls back to `ctest -N` text |

---

## Project layout

```
lazycmake/
├── crates/
│   ├── lazycmake-core/   # presets, File API, ctest, config, env files
│   └── lazycmake-tui/    # ratatui UI + `lazycmake` binary
├── examples/
│   └── config.toml       # annotated sample config
├── Cargo.toml            # workspace
└── README.md
```

### Design notes

- **Threads, not async** — one job at a time; UI thread + job thread over channels
- **No shell for cmake/ctest** — `std::process::Command` only (`env_file` uses bash solely to source the script)
- **Generator-aware** — Ninja / multi-config / Make / VS / Xcode affect `--config` and artifact paths

---

## Development

```bash
# Unit tests (no live cmake required for core fixtures)
cargo test --workspace --locked

# Run against a real project
cargo run -p lazycmake-tui --locked -- -C /path/to/project --config /path/to/config.toml
```

### CI

| Workflow | When | What |
|----------|------|------|
| [`ci.yml`](.github/workflows/ci.yml) | push / PR | `cargo test` on Linux, macOS, Windows |
| [`release.yml`](.github/workflows/release.yml) | tag `v*` | release builds for 5 targets + GitHub Release assets |

Core tests use checked-in JSON fixtures for File API / ctest parsing. Process I/O helpers are covered with small shell snippets.

---

## Troubleshooting

| Symptom | Likely cause |
|---------|----------------|
| Presets column empty | CMake &lt; 3.19, or no `CMakePresets.json` |
| Targets empty after configure | File API query missing / configure failed; check Output (`o`) |
| `$env{VAR}` error on configure | Variable unset; set it or use `env_file` on that preset’s override |
| Tests stay `-` after `T` | Parse/status bug or truncated log — check Output for `100% tests passed…` |
| `env_file` fails | Path wrong after expansion; script must be valid bash; need `bash` on `PATH` |
| UI garbled after a job | Should be rare; jobs are detached from the TUI tty — please file an issue with CMake version and steps |

---

## Roadmap

Ideas under consideration:

- Quick-configure when no presets file exists  
- Build / test presets from `CMakePresets.json`  
- Custom target entries in config (`[custom_targets.*]`)  
- Watch mode, mouse support, themes  

Contributions and issue reports are welcome.

---

## License

[MIT](LICENSE) © the lazycmake contributors.
