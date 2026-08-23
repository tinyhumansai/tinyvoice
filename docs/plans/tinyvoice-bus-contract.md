# Implement TinyVoice bus contract

Linked specification: [`../specs/tinyvoice-bus-contract.md`](../specs/tinyvoice-bus-contract.md)

1. Move workspace packages under `crates/` and isolate the dynamic module.
2. Add the transport-free `tinyvoice-bus` vocabulary crate.
3. Re-export shared serialized types from the pure library and consume bus
   identity values in the module.
4. Update documentation, CI, release paths, and run both workspace suites.
