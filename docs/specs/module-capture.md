# Module-owned recording contract

TinyVoice contract 1.1 adds six members while preserving every existing method's
arity and wire format:

| Member | One argument | Result |
| --- | --- | --- |
| ListInputDevices | None | CaptureResult of device names |
| RecordingStart | RecordingStartRequest | CaptureResult of CaptureHandle |
| RecordingFinish | RecordingFinishRequest | CaptureResult of AudioOutput |
| RecordingCancel | CaptureHandle | CaptureResult of unit |
| ReadAudioOutput | ReadAudioRequest | CaptureResult of base64 WAV bytes |
| ReleaseAudioOutput | CaptureHandle | CaptureResult of unit |

ListInputDevices has zero arguments. Each other new member takes exactly one.
The contract contains serialized vocabulary only; CPAL, native device threads,
processing, storage and entropy remain in the implementation module.

The host obtains the microphone decision through TinyComputer and passes an
explicit Granted value. Denied is the default and fails before device access.
The module preserves the library's dedicated-thread native capture behavior.

Only one recording or finishing operation can own the device slot. Start returns
a random opaque handle. Finish consumes it and waits for capture to stop before
downmixing, resampling to 16 kHz, optional silence gating and PCM16 WAV encoding.
Cancel consumes the recording and waits for device shutdown, discarding its
audio and capture errors. Invalid gate thresholds retain the lease so it can
be canceled. Setup, processing and storage failures release device capacity.
Dropping the module drops active library handles, signaling their device threads
to stop.

Prepared outputs have distinct opaque handles and require explicit release.
There are at most four outputs totaling 32 MiB. Reads return at most 256 KiB
per call; EOF returns an empty string. Unknown, consumed and released handles
fail explicitly. Preparation checks expanded sample count before resampling
and checks encoded and aggregate size afterward. No transport call occurs per
audio sample.

CaptureError is a tagged serialized result. Device detail supports product
presentation only. Host telemetry must use safe reason codes and omit device
names, error detail, handles, audio, paths and request arguments.

Local fixtures exercise lifecycle, bounds, faults and in-memory bus dispatch
without hardware or external services. The actual compiled artifact verifier
checks all 21 declared members and permission denial without opening a device.
The native device bridge follows the repository's existing physical-device
coverage exception; manager and processing code remain subject to the 90%
per-file threshold.

The release workflow installs ALSA development headers for Linux module builds.
OpenHuman must pin a released artifact compatible with contract 1.1 and verify
its digest before switching callers. Continuous capture and hotkey lifecycle
operations are follow-up work; this change covers enumeration and one-shot
recording only.
