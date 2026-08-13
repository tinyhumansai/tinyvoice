# TinyBus Adapter

This module is the boundary between ordinary feature code and TinyBus module
ABI v1. `GreetingService` converts the crate's public `greet` function into the
typed `Greet` bus method, while `setup` registers its object and claims the
well-known interface name.

`tinybus_module::module_export!` emits the descriptor, embedded manifest, and
initialization symbols consumed by the dynamic loader. The manifest method list
must stay aligned with the interface macro's dispatch table; the unit test
checks that relationship. Integration tests use TinyBus's in-memory transport,
and `examples/verify_module.rs` loads a compiled `cdylib` through the real
dynamic loader before a release archive is accepted.

Generated projects should replace the example interface, object path, and
method declarations together. They must not retain Rust-owned data across the
ABI boundary or bypass the SDK exports with an ad hoc FFI surface.
