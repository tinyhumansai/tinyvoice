# Implement module-owned hotkey lifecycle

Specification: [`../specs/hotkey-module-lifecycle.md`](../specs/hotkey-module-lifecycle.md)

## Goal and assumptions

Add the six approved TinyVoice 1.4 hotkey members without changing current library behavior or host adapters. TinyComputer GlobeRead/GlobeShutdown is published at contract 2.12; Voice receives only generic sequenced FN facts. Linux implementation targets X11. No root gitlink, release pin, or host migration is part of this source slice.

## Tasks

1. **Pin activation semantics.** Add module tests for current key aliases, left/right modifiers, trigger-first/modifier-first, repeat press, tap toggle, push trigger release, and push modifier release. Confirm each test fails if the corresponding expected transition is inverted or omitted. Extract or expose the smallest pure reducer from `tinyvoice` if that avoids duplicating its existing behavior.
2. **Prototype and own Linux X11 cancellation.** Under ignored `target/`, keep the Xvfb RECORD prototype and its output. Add a native owner seam with startup readiness, bounded event handoff, cancellation handle, cleanup result, and join handle. Add fixture tests for cancellation while the record iterator is blocked, failed cleanup retaining ownership, retry, and shutdown waiting for join. Only after these regressions pass, implement the x11rb RECORD backend using the validated duplicated-FD shutdown path and physical keycode mapping.
3. **Add pure TinyVoice 1.4 vocabulary.** Add the six method names and request/status/error/event types under `crates/tinyvoice-bus/src/hotkey.rs`; add exact serde tests and bump `CONTRACT_VERSION` to `(1, 4)`. Keep names/catalogue and manifest method list synchronized.
4. **Build the listener lease manager.** Add bounded reservation, idempotent start, replayable Read snapshots, generation-fenced host Feed, event-sequence gap reset, retryable Stop, terminal Shutdown, and injected backend fixtures under `crates/tinyvoice-module/src/service/hotkey/`. Each state transition gets a failing fixture first. Run in-memory TinyBus wire tests after each contract/member change.
5. **Implement Windows ownership.** Add the owner-thread message loop, keyboard-only low-level hook, bounded callback handoff, owner-thread unregister, and joined shutdown. Use local fake/native-owner fixtures for startup race, cancellation, failed unregister, and retry. If the Windows binding requires unsafe code, isolate it in the one documented FFI file and explicitly limit the lint allowance there.
6. **Implement host-fed macOS source.** Reject native macOS registration. Accept only bounded typed FN facts for the generation returned by Start; pin duplicate, stale generation, lost reply, sequence gap, overflow reset, and next-release rearm behavior. Do not add a TinyComputer dependency.
7. **Verify and document the artifact.** Add the operations to module/manifest artifact verification and docs; test exact member arities/wires. Run library and module fmt, strict clippy, all-target builds, default/all tests, strict docs, pure dependency closures, and per-file coverage. Rebuild and run the actual module verifier. Save RED/GREEN logs, gate outputs, coverage, final diff, and artifact digest under ignored `target/`, then freeze the source head for independent review before extending canonical PR23.

## Verification commands

Run long commands from the TinyVoice owner root through `../scripts/ci-cancel-aware.sh` when available (otherwise the repository's equivalent cancel-aware helper):

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all-targets --all-features
cargo test --all-features
cargo test --manifest-path crates/tinyvoice-module/Cargo.toml --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
```

Also run default tests, the module artifact verifier, dependency-closure checks for bus default/all features, and the per-file 90% coverage gate. Keep all command logs and built artifacts under the owner's `target/` tree.

## Completion checklist

- [ ] Linux cancellation prototype and owner fixtures pass.
- [ ] Contract and manifest both report 1.4 and the same method list.
- [ ] Linux, Windows, and macOS-feed behaviors meet the accepted specification.
- [ ] Stop/shutdown tests prove actual cleanup and join; fault tests prove retryability.
- [ ] Library and module gates, coverage, and actual artifact verification pass.
- [ ] Frozen diff/evidence is independently reviewed before PR23 extension.
