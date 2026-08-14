# TinyBus module

Status: Implemented

Owner: TinyVoice maintainers

## Problem

TinyVoice must be installable as a compiled TinyBus module so a host can use
document generation without linking the document stack into its own binary.
The released artifact must exercise the same ABI and loading path used in
production.

## Goals

- Preserve the existing pure Rust library API.
- Build a target-specific dynamic library implementing TinyBus module ABI v1.
- Expose typed DOCX generation through a stable bus identity.
- Publish installable native bundles with each GitHub release.
- Test loading and calling the compiled artifact through a real broker.

## Non-goals

- Loading untrusted third-party modules safely.
- Stable ABI compatibility across TinyBus ABI revisions.
- Streaming or file-descriptor transfer in this first interface.
- Running TinyVoice as a separate socket process.

## Behavior

The private `tinyvoice-module` workspace crate depends on the public library's
`docx`, `pptx` and `pdf` features and builds as a `cdylib`. This separation keeps
unpublished, vendored TinyBus packages out of the crates.io package manifest. The
module claims `ai.tinyhumans.tinyvoice.Documents`, serves the object path
`/ai/tinyhumans/tinyvoice/Documents`, and exports five methods:

```text
GenerateDocx(DocumentSpec)              -> OutputRef
GeneratePptx(deck, Option<StreamRef>)   -> OutputRef
ExtractText(StreamRef)                  -> OutputRef
ReadOutput(output_id, offset, len)      -> base64
ReleaseOutput(output_id)                -> ()
```

The format arguments are the same Serde contracts used by the Rust API, except
that a slide image declares its length in the concatenated image stream rather
than carrying bytes inline.

Inbound payloads ride TinyBus streams, so flow control, the size cap and the
idle timeout are the bus's. A deck's images share one stream because a call has
one stream; their declared lengths are the authority on where each image ends.

Replies cannot stream: `Interface::call` receives no caller identity and no
connection, so a served object cannot open a stream back to its caller. A
produced document is therefore held and pulled with `ReadOutput`, because a
frame is a 16 MiB JSON document and a `Vec<u8>` serialises as an array of
integers — about 3.5 bytes of frame per byte.

Invalid input, writer failures and extraction failures use the distinct wire
names `ai.tinyhumans.tinyvoice.Error.InvalidInput`,
`ai.tinyhumans.tinyvoice.Error.GenerationFailed` and
`ai.tinyhumans.tinyvoice.Error.ExtractionFailed`. Transfer failures are grouped by
what the caller should do next: `Error.UnknownOutput` (the document is gone;
make the call again), `Error.OutputRefused` (a budget is full; the same request
may succeed later) and `Error.TransferFailed` (the read was malformed, or an
inbound stream did not complete).

Synthesis and extraction are CPU-bound and run on the module runtime's blocking
pool. The module retains no document state between calls — only produced documents
waiting to be read, each bounded and expiring.

This interface replaces `ai.tinyhumans.tinyvoice.Docx`, which returned bytes
inline. TinyBus forbids changing an interface in place, so the new contract took
a new name. It is not served alongside the old one: `module_export!` attaches its
method list to the first entry in `provides` and leaves the rest empty, so a
second fully-declared interface is not expressible without a TinyBus change.

## Invariants and constraints

- The vendored TinyBus gitlink is the ABI source of truth.
- Manifest methods and generated dispatch members must remain identical.
- No Rust value crosses the dynamic-library ABI boundary.
- The native artifact must match the host target and TinyBus compatibility
  gate.
- Message payloads remain subject to TinyBus's 16 MiB frame cap. Inbound bytes
  avoid it through streams; outbound bytes are pulled in bounded chunks until
  TinyBus gains a reply-stream seam.
- Held documents are bounded per document, in total, by count, and by an idle
  TTL. A module is never unloaded, so an unbounded store is a leak with no end.
- An image stream that does not match the lengths the deck declares is refused,
  so a truncated transfer cannot become a deck with a corrupt picture in it.
- Dynamic modules are trusted code with the host process's privileges.

## Acceptance criteria

- `cargo build --release --package tinyvoice-module` emits the platform dynamic
  library.
- TinyBus `ModuleHost` admits that artifact and reaches `ready` state.
- `GenerateDocx` and `GeneratePptx` stage output beginning with the OOXML `PK`
  signature, and `ExtractText` recovers the text layer of a staged PDF.
- An image transferred across more than one chunk arrives intact and is embedded,
  which is the case a single-chunk transfer would not prove.
- CI executes that loader test on Linux.
- A release uploads Linux and macOS bundles containing the matching TinyBus
  host, TinyVoice module, SHA-256 allowlist, and operational documentation.
- A release uploads `checksum.toml` with the SHA-256 digest of every archive so
  TinyBus can verify a precompiled module before extracting or loading it.
- The release also uploads the crates.io package and pinned TinyBus source.

## Open questions

None blocking this version.

Two things belong upstream in TinyBus rather than here.

A reply-stream seam would delete the output store entirely: the only reason a
produced document is held at all is that a served object cannot open a stream
back to its caller.

And `module_export!` attaching its method list only to the first provided
interface is what forces one interface to carry both the output methods and the
format methods; per-interface method lists would allow the cleaner split.
