# Rust Template TinyBus Module

This package contains the native `rust-template` module for TinyBus module ABI
v1. Install only the archive matching the host operating system and
architecture.

The module claims `ai.tinyhumans.rust_template.Greeting`, serves the object at
`/ai/tinyhumans/rust_template/Greeting`, and provides the `Greet` method. The
method accepts one string and returns `Hello, <name>!`; empty names are rejected.

The archive contains one `.so`, `.dylib`, or `.dll` plus `modules.toml`. Keep
those files together when copying them into a TinyBus module directory. The
allowlist binds the native library filename to its SHA-256 digest so TinyBus can
reject a missing, renamed, or modified artifact before initialization.

The GitHub release also publishes `checksum.toml` as a separate asset. TinyBus
checks that manifest before downloading and extracting the selected platform
archive. Install directly from a tagged release with:

```sh
tinybus modules load-github \
  https://github.com/tinyhumansai/rust-template/releases/tag/v0.1.4 \
  rust-template-0.1.4-ubuntu-24.04-x86_64.tar.gz \
  <archive-sha256>
```

TinyBus modules are trusted in-process code. Install release artifacts only
from a trusted source and restart the host after replacing a loaded module.
