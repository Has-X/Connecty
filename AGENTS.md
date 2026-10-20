# Repository Guidelines

## Project Structure & Module Organization

- `src/`: C++20/C++/WinRT frontend; `App.cpp` contains WinUI 3 views, `Core.*` logic and IPC integration.
- `agent/src/`: Rust background agent. Keep SSH transport in `ssh`, lifecycle ownership in `manager`, IPC in `ipc`, and Windows integration in `platform`, `startup` and `supervisor`.
- `tests/CoreTests.cpp`: C++ core checks; Rust unit tests live beside implementation in `#[cfg(test)]` modules.
- `scripts/`: build, packaging and integration checks. `assets/` contains branding and Windows resources; `installer/` contains Inno Setup sources; `packaging/` contains runtime pins and WinGet manifests.
- `docs/`: architecture, installation, testing and validation. Generated outputs belong in ignored `build/`, `dist/` and `agent/target/`.

## Build, Test, and Development Commands

Use Windows 11, Rust's MSVC toolchain, Visual Studio 2022 desktop C++ tools, CMake/Ninja and a Windows SDK with C++/WinRT. Run from the repository root in PowerShell:

```powershell
./scripts/build.ps1 -Test -Package -Architecture x64
./scripts/test-ui.ps1 -Architecture x64
./build/native/Connecty.exe
cargo test --manifest-path agent/Cargo.toml --locked
cargo fmt --manifest-path agent/Cargo.toml -- --check
cargo clippy --manifest-path agent/Cargo.toml --locked --all-targets -- -D warnings
```

The build script restores dependencies, builds binaries, runs checks and packages a ZIP. The UI check validates XAML startup. Use `arm64` on matching hardware for tests. Build installers with `scripts/build-installer.ps1`; Inno Setup 7.1.0+ is required. Change releases only in `[package].version` in `agent/Cargo.toml`; builds refresh derived metadata and `Cargo.lock`.

## Coding Style & Naming Conventions

Follow `.editorconfig`: UTF-8, LF, four spaces; YAML uses two spaces. Format C++ with `.clang-format` (Microsoft style, 110-column limit, preserved include order). Use `cargo fmt` for Rust. Follow existing C++ PascalCase types/functions and camelCase fields; Rust uses snake_case functions/modules and PascalCase types.

## Testing Guidelines

Use Rust's built-in tests and CTest's `core` check. Name Rust tests after the behavior or regression. Add focused tests for lifecycle, trust, storage and IPC changes. Run `scripts/test-supervisor.py` for process recovery; use fresh, isolated data directories. Follow `docs/testing.md` for opt-in live SSH faults. Distinguish simulated recovery from physical sleep and short checks from soak testing.

## Commit & Pull Request Guidelines

Use short, imperative commit subjects; history also uses `docs:` for documentation. Keep commits focused. PRs should explain the problem, behavior change and validation, link relevant issues, and include screenshots for UI changes. State untested architectures or runtime scenarios.

## Security & Architecture Constraints

Keep SSH embedded in Rust/Tokio/russh; preserve event-driven recovery, bounded logs and authenticated named pipes. Never bypass host-key verification, persist passphrases or commit personal profiles, keys or logs. Preserve profile compatibility and UI-independent operation. Report vulnerabilities through `SECURITY.md`.
