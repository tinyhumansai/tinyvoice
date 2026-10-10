# Module-owned hotkey lifecycle

**Status:** Accepted
**Owner:** TinyVoice module and bus

## Problem

The current rdev listener runs in a detached process thread. `stop` only suppresses callback output, Drop detaches the thread, and the process-wide listener guard prevents reconnect. That lifetime cannot safely be acknowledged by a dynamically loaded module or used across sign-out and listener replacement.

## Goals

- Run hotkey acquisition and activation inside the compiled TinyVoice module.
- Preserve supported key aliases, left/right modifiers, key repeat handling, and tap/push behavior.
- Bound listener reservations, native input queues, bus feed batches, and replay batches.
- Make startup idempotent after a lost reply, and make Stop/Shutdown success mean native callback registration is removed and its worker has joined.
- Support Linux X11 and Windows native listeners with local injectable lifecycle fixtures.
- Keep macOS input on the host-owned TinyComputer Globe listener and accept only bounded sequenced activation facts over the generic TinyVoice bus vocabulary.
- Keep `tinyvoice-bus` transport-free and free of device, runtime, native-key, and parsing behavior.

## Non-goals

- Adding native input behavior to the host or directly linking TinyComputer accessibility into TinyVoice.
- Global Wayland hotkeys; Linux X11 support does not imply Wayland support.
- Changing the existing pure-library `ActivationMode` snake_case representation or removing the compatibility `tinyvoice::hotkey` API.
- Registering permissions or deciding whether hotkey use is allowed; the host obtains platform permission through TinyComputer.
- Polling for each key sample or forwarding key names/native key codes over TinyBus.

## Proposed bus surface

Contract 1.4 adds six members:

| Member | Input | Result |
| --- | --- | --- |
| `HotkeyReserve` | bounded `HotkeyRequest` with key text, activation mode, and source (`native` or `host`) | opaque `HotkeyHandle` |
| `HotkeyStart` | reserved handle | status plus a feed generation when host-fed |
| `HotkeyRead` | handle plus optional acknowledged batch | bounded replayable ordered Pressed/Released batch and status |
| `HotkeyFeed` | handle, generation, sequence, bounded Down/Up facts, and optional upstream-loss flag | feed acknowledgment/status |
| `HotkeyStop` | handle | status only after release and join |
| `HotkeyShutdown` | no arguments | terminal cleanup status only after every owned worker/resource is released |

New enums use explicit snake_case serde names. The activation mode remains `tap`/`push`. Host feed is generic fact vocabulary, not a Computer handle or Computer DTO. Start returns a module-owned generation; stale generations reject. Sequence duplicates replay their prior acknowledgment, gaps mark continuity lost, and a reported upstream overflow resets activation inactive. Consumers must observe a fresh physical release before rearming after loss.

`HotkeyRead` keeps the oldest-first batch until its exact batch number is acknowledged. A lost response is replayed unchanged; future acknowledgments fail without mutation. A continuity reset does not invalidate an already-returned batch: its exact acknowledgment is accepted before the next read returns the reset snapshot. Queue overflow does not silently drop a release while preserving active state: the activation reducer resets inactive and the batch reports loss. Requests, strings, event batches, retained batches, reservations, and active listeners all have fixed documented limits. Released handles do not consume a lifetime admission counter.

The module admits at most 16 live reservations, expires an unused reservation
after 60 seconds, and accepts key strings up to 128 UTF-8 bytes and opaque
handles up to 32 bytes. Each host feed and replay batch contains at most 64
facts; the native and retained event queues hold at most 256 events. Reaching
an event-queue bound reports continuity loss and resets activation inactive.

`HotkeyStop` is retryable after cleanup failure: the module retains the handle and native owner until unregistration and join complete. `HotkeyShutdown` closes admission first, cancels startup and waits for all listeners. The host calls it before generic ABI unload. Sign-out and replacement await per-handle Stop before starting a successor.

## Platform behavior

- **Linux/X11:** use the X RECORD extension over two owned x11rb connections. The data connection's duplicated socket descriptor is retained as a safe cancellation handle; shutdown wakes a blocked record iterator. The worker owns its X11 state and joins before Stop succeeds. Keycodes map to the same physical key vocabulary as the existing listener. Press/release, repeat, and modifier ordering are pinned with local fixtures. Wayland remains explicitly unsupported.
- **Windows:** one owner thread creates its message queue, installs `WH_KEYBOARD_LL`, and pumps messages. Its callback only maps key transitions into a bounded nonblocking queue. Unexpected message-loop termination marks continuity lost; the owner stays reachable after failed unregistration so cleanup can be retried. Stop unregisters the hook, clears the callback target, and joins before success. Unsafe stays limited to the documented TinyVoice module FFI boundary; workspace and pure library unsafe policies remain forbidden.
- **macOS:** the module does not start rdev or another accessibility hook. The host owns TinyComputer Globe permission and lease, reads its reliable batches, and forwards sequenced FN Down/Up facts. Host sign-out awaits both TinyVoice Stop and Computer Stop.
- **Other targets:** return an explicit unsupported result without creating native resources.

## Invariants

- No native thread retaining module code survives a successful terminal shutdown.
- No callback performs parsing, blocking sends, bus calls, audio work, or logging of key payloads.
- Start reply loss is recoverable using the caller-known reservation; late workers cannot publish to retired handles.
- Stop and shutdown never report success before actual hook unregistration/socket teardown and worker join.
- A failed cleanup keeps ownership reachable for retry.
- Every continuity break resets activation state before new facts are interpreted.
- Only typed Pressed/Released facts and opaque handles cross the bus; native keys, connection state, parsers, and key-state algorithms stay in TinyVoice.

## Acceptance criteria

- Deterministic local tests cover aliases, tap/push transitions, left/right modifiers, repeat events, modifier release, ordering, bounded input/output, replay, acknowledgment, startup reply loss, cancellation, stale generations, sequence gaps, overflow reset, cleanup retry, sign-out, and reconnect.
- A Linux Xvfb prototype and local protocol fixture prove a blocked X RECORD reader wakes and joins when its duplicated socket is shut down.
- Native owner fixtures prove unregister-before-join and no successful acknowledgment after cleanup failure. Tests require no physical keyboard, display permission, backend, or network service.
- Default/all-feature bus vocabulary remains free of native/runtime dependencies; both module and library contract gates pass.
- The actual compiled artifact verifies all declared hotkey members and exercises denied, stale, replay, stop, and terminal paths.
- Hosts do not switch callers or pin the module until the source is accepted and a compatible artifact is released.

## Open questions

None blocking. The exact safe Windows API surface and Linux wire parsing are implementation details constrained by the invariants above; if a local native prototype disproves an assumption, revise the implementation plan and raise the smallest affected contract decision before proceeding.
