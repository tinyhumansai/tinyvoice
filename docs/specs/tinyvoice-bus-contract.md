# TinyVoice bus contract

Status: Implemented

## Problem

A host loading the TinyVoice dynamic module needs its member names and value
types, but must not compile TinyBus, its runtime, or the implementation.

## Behavior

`crates/tinyvoice-bus` is the transport-free contract crate. It owns the bus
name, object path, ordered method constants, contract version, and serialized
voice values (`VadConfig`, `VadEvent`, `VoiceIntent`, and `Mode`).
`crates/tinyvoice` depends on and re-exports those value types at its existing
module paths, so the in-process and bus-facing APIs use identical definitions.

The loadable module is an independent Cargo workspace at
`crates/tinyvoice-module`; it depends on both the pure library and the bus
contract, and asserts its served members against `tinyvoice_bus::METHODS`.

## Constraints

- `tinyvoice-bus` has no TinyBus, runtime, I/O, or host dependency.
- Existing `tinyvoice::{vad, intent, transcript}` type paths remain valid.
- Cargo commands for the module use its manifest explicitly.

## Acceptance criteria

- A host can depend on `tinyvoice-bus` alone to name the interface and values.
- The pure library and module compile and test from their respective roots.
- The module's served member list remains equal to `tinyvoice_bus::METHODS`.
