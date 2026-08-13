# Rust Template

A production-ready Rust 2024 TinyBus module template used by TinyHumans AI. It
ships the module layout, TinyBus ABI adapter, error handling, testing,
documentation, CI, and multi-platform release workflow that every new
integration in this organization starts from.

## Use This Template

Choose **Use this template** on GitHub, create a repository, then work through
the checklist at the top of [`AGENTS.md`](AGENTS.md):

- update the package name, description, repository, keywords, and categories in
  `Cargo.toml`;
- update this README and the crate documentation in `src/lib.rs`;
- replace the placeholder `greeting` module with the first real feature area;
- rename the TinyBus interface, object path, and exported methods in
  `src/tinybus_module/`;
- update the security contact and repository links in the community files;
- replace `ROADMAP.md` with the real plan, or delete it;
- change the license if GPL-3.0-only is not appropriate.

Search for `rust-template` and `rust_template` to find every remaining
template-specific value.

## What You Get

| Area | What is configured |
| --- | --- |
| Layout | Directory modules with `mod.rs` / `types.rs` / `test.rs`, a crate-wide error type, integration tests, and a runnable example |
| Lints | `unsafe_code` forbidden, `missing_docs`, clippy `all` + `pedantic`, no `unwrap`/`expect`/`panic`/`todo` in library code — all declared in `[lints]` so local and CI runs agree |
| CI | Format, clippy, build, test (default and all features), at least 90% line coverage in every source file, rustdoc with `-D warnings`, an MSRV build, and a `cargo-deny` supply-chain check |
| Release | Manual `workflow_dispatch` bump that validates, versions, tags, and creates installable native module packages for every supported platform |
| Community | Issue and pull request templates, Dependabot, contributing, security, support, and code of conduct docs |
| Agents | [`AGENTS.md`](AGENTS.md) as the single source of truth, symlinked as `CLAUDE.md`, plus a `.claude/settings.json` allowlist for the standard commands |
| Vendor | TinyBus host types and module SDK pinned as the `vendor/tinybus` build-time submodule |

## Layout

```text
src/
├── lib.rs              # crate docs + the entire public re-export surface
├── error/
│   ├── mod.rs          # crate-wide `Error` and `Result<T>`
│   └── test.rs
├── greeting/           # one directory per feature area
    ├── mod.rs          # module docs, wiring, smallest useful public API
    └── test.rs         # module-local unit tests
└── tinybus_module/
    ├── mod.rs          # bus interface, setup, and ABI v1 exports
    └── test.rs         # real in-memory TinyBus integration tests
tests/
└── public_api.rs       # integration tests against the public API only
examples/
├── basic.rs                    # ordinary library API usage
├── verify_module.rs            # local dynamic-module verification
└── verify_github_release.rs    # tagged-release download and bus call
vendor/
└── tinybus/            # pinned TinyBus git submodule
docs/
├── README.md           # documentation index and conventions
├── specs/              # behavior and architecture specifications
├── plans/              # implementation-ordered delivery plans
└── adr/                # immutable architecture decision records
```

Feature areas use directory modules: implementation and exports live in
`mod.rs`, substantial types move to `types.rs`, and unit tests live in
`test.rs`. [`AGENTS.md`](AGENTS.md) holds the complete repository guidance, and
`CLAUDE.md` is a symlink to it so every coding agent reads one source of truth.

## Development

Clone with submodules, or initialize them before building:

```sh
git submodule update --init --recursive
```

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all-targets --all-features
cargo test --all-features
cargo run --example basic
cargo build --release --lib            # produces the installable cdylib
```

Those four checks are exactly what CI runs. Optional extras:

```sh
cargo doc --no-deps --all-features   # CI builds this with RUSTDOCFLAGS="-D warnings"
cargo deny check all                 # supply-chain check; see deny.toml
cargo install cargo-llvm-cov         # once, before running the coverage gate
.github/scripts/check-file-coverage.sh 90 target/coverage.json
```

## Releasing

Run the **Release** workflow from the Actions tab with a `patch`, `minor`, or
`major` bump. Use `current` only to resume an interrupted release whose version
commit and tag already exist. The workflow revalidates the crate, versions and
tags it, builds this crate as a TinyBus `cdylib`, and creates a GitHub release.
Assets follow `rust-template-<version>-<platform>.<tar.gz|zip>` and contain the
native module, its SHA-256 `modules.toml`, license, and
[`MODULE.md`](MODULE.md). Every release also publishes `checksum.toml`, which
TinyBus uses to verify an archive before extraction. The workflow loads the
published Ubuntu archive through TinyBus's GitHub release API and calls its
`Greet` method before declaring the release successful. TinyBus itself is not
shipped by this repository; the pinned submodule is the build-time SDK. The stable native
matrix covers Ubuntu 22.04 and 24.04 on x86_64 and ARM64; Fedora 43 and 44 on
x86_64 and ARM64; rolling Arch Linux on its officially supported x86_64
architecture; macOS 15 and 26 on Intel and Apple Silicon; Windows Server 2022
and 2025 on x86_64; and Windows 11 on ARM64. Preview, deprecated, and unofficial
architecture images are not release gates. Do not hand-edit the version in
`Cargo.toml`.

## Documentation

- [`AGENTS.md`](AGENTS.md) — repository guidelines for humans and agents
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to propose a change
- [`docs/specs/`](docs/specs/README.md) — behavior and architecture specs
- [`docs/plans/`](docs/plans/README.md) — test-first implementation plans
- [`docs/adr/`](docs/adr/0001-record-architecture-decisions.md) — architecture
  decision records
- [`SECURITY.md`](SECURITY.md) — how to report a vulnerability

## License

GPL-3.0-only. See [LICENSE](LICENSE).
