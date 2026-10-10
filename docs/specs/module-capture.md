# Module-owned native capture contract

TinyVoice contract 1.3 adds eleven members while preserving every existing method's
arity and wire format:

| Member | One argument | Result |
| --- | --- | --- |
| ListInputDevices | None | CaptureResult of device names |
| CaptureShutdown | None | CaptureResult of unit |
| ReserveCapture | RecordingStartRequest (permission only) | CaptureResult of CaptureHandle |
| RecordingStart | RecordingStartRequest | CaptureResult of CaptureHandle |
| RecordingFinish | RecordingFinishRequest | CaptureResult of AudioOutput |
| RecordingCancel | CaptureHandle | CaptureResult of unit |
| ReadAudioOutput | ReadAudioRequest | CaptureResult of base64 WAV bytes |
| ReleaseAudioOutput | CaptureHandle | CaptureResult of unit |
| CaptureStart | RecordingStartRequest | CaptureResult of CaptureStream |
| CapturePoll | CapturePollRequest | CaptureResult of CaptureBatch |
| CaptureStop | CaptureHandle | CaptureResult of unit |

ListInputDevices and CaptureShutdown have zero arguments. Each other new member takes exactly one.
ReserveCapture returns a module-generated handle before device startup. The host
passes this known handle in RecordingStartRequest when starting either native mode.
Missing handles are refused. The contract contains serialized vocabulary only; CPAL, native device threads,
processing, storage and entropy remain in the implementation module.

The host obtains the microphone decision through TinyComputer and passes an
explicit Granted value. Denied is the default and fails before device access.
The module preserves the library's dedicated-thread native capture behavior.

Only one recording, continuous stream or finishing operation can own the device slot. ReserveCapture returns
a random opaque handle. Idle reservations do not claim the device slot; at most 16
are retained, and they expire after 60 seconds. Expiry is checked on reserve/start.
Cancel or stop can discard an idle reservation. Once setup begins, cancellation
marks the known reservation and waits for native setup and cleanup. Startup
workers reclaim resources if their result receiver disappears. Finish consumes it and waits for capture to stop before
downmixing, resampling to 16 kHz, optional silence gating and PCM16 WAV encoding.
Cancel consumes the recording and waits for device shutdown, discarding its
audio and capture errors. Invalid gate thresholds retain the lease so it can
be canceled. Setup, processing and storage failures release device capacity.
Native startup, finish, cancel and stop run in owned blocking workers. Dropping a
request future does not release its BusyGuard while cleanup/preparation is pending.
Workers retain ownership through completion even if a request future disappears.
CaptureShutdown closes this capture instance permanently, marks pending startup
and preparation canceled, discards outputs, drains live resources and awaits all
native cleanup and worker completion. Reserve/start/finish then return Closed.
The host must invoke CaptureShutdown before the generic TinyBus ABI shutdown: its
runtime.shutdown_timeout alone does not await blocked native workers. Sign-out
uses per-handle cancel/stop/release so the process module remains usable. Dropping
the module drops idle/active library handles, signaling their device threads to stop.

Prepared outputs retain the caller-known recording handle and require explicit release.
A host can release that handle while preparation is pending or after losing the
finish reply; pending release discards the eventual result.
There are at most four outputs totaling 32 MiB. Reads return base64 for at most 256 KiB of WAV bytes
per call; EOF returns an empty string. Unknown, consumed and released handles
fail explicitly. Preparation and legacy PrepareFrames/PrepareCapture check expanded sample count before resampling
and checks encoded and aggregate size afterward. No transport call occurs per
audio sample.

CaptureError is a tagged serialized result. Device detail supports product
presentation only. Host telemetry must use safe reason codes and omit device
names, error detail, handles, audio, paths and request arguments.

Local fixtures exercise lifecycle, bounds, faults and in-memory bus dispatch
without hardware or external services. The actual compiled artifact verifier
checks all 32 declared members and permission denial without opening a device.
The native device bridge follows the repository's existing physical-device
coverage exception; manager and processing code remain subject to the 90%
per-file threshold.

The release workflow installs ALSA development headers for Linux module builds.
OpenHuman must pin a released artifact compatible with current contract 1.4
(this capture slice was introduced in 1.3) and verify its digest before
switching callers. Module-owned hotkeys are specified in
`hotkey-module-lifecycle.md`.

Continuous capture returns an opaque handle and native sample rate/channel
format. Native callbacks forward interleaved f32 samples through an eight-chunk
queue without blocking. Full queues and callbacks exceeding 32,768 samples drop
the newest callback; polling drains one or two chunks in order, reports whether
the sender has closed, and rejects larger batches. Stop consumes the handle,
waits for native thread cleanup, and reports any terminal device failure. A
failed setup or stop releases device capacity so callers can restart. Dropping
the manager requests shutdown; closing the consumer also ends native capture.
Neither stream polling nor device callbacks run STT, VAD or product policy.
