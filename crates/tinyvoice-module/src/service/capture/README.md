# Native capture ownership

`Capture` owns one microphone slot shared by recording and continuous streams.
Each live resource holds a `BusyGuard`; owned blocking workers retain it through
native cleanup/preparation even if the requesting future disappears.
`Backend`, `Recording` and `Stream` allow lifecycle fixtures without hardware.
The production bridge in `device_native.rs` opens CPAL through the library's
existing dedicated thread behavior only after the host supplies permission.

The host first reserves an opaque handle, then uses it for startup and cancellation.
Sixteen idle reservations expire after 60 seconds and hold no microphone capacity.
Pending cancellation awaits setup and cleanup; startup receiver loss also cleans up.

Recording leases become bounded prepared WAV outputs under the same known handle
when finished. Release works while preparation is pending as well as after delivery. Cancel
waits for device cleanup and discards samples. Invalid processing parameters
retain the recording lease so callers can retry or cancel.

Continuous leases use an eight-chunk native queue. Polling drains at most two
chunks without waiting, preserving native channels, sample rate and ordering.
The library drops callbacks larger than 32,768 samples and drops the newest
callback when the queue is full. Stop removes the lease, awaits thread cleanup
and reports terminal failures; drop requests shutdown without waiting.

Mutex poisoning fails closed. Unknown and consumed handles fail explicitly.
Opaque random handles prevent accidental rebinding across module reloads.
Tests exercise permission denial, exclusivity, bounds, restart, failed setup,
failed stop, manager drop, serialization and poisoned tables.

CaptureShutdown is terminal: it freezes startup publication, cancels pending
preparation, discards outputs and awaits native workers. Hosts call it before
TinyBus ABI shutdown; runtime.shutdown_timeout alone cannot guarantee cleanup
of blocked native setup. Sign-out uses cancel/stop/release instead.
