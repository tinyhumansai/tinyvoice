# Native capture ownership

`Capture` owns one microphone slot shared by recording and continuous streams.
Each live resource holds a `BusyGuard`; failures and drop release the slot.
`Backend`, `Recording` and `Stream` allow lifecycle fixtures without hardware.
The production bridge in `device_native.rs` opens CPAL through the library's
existing dedicated thread behavior only after the host supplies permission.

Recording leases become bounded prepared WAV outputs when finished. Cancel
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
