# Native recording ownership

The capture manager owns a single active device lease, including finish and
preparation, and up to four WAV outputs totaling 32 MiB. Permission must be an
explicit host grant obtained through TinyComputer before the native backend is
called. The backend preserves the library's dedicated-thread CPAL lifecycle.

RecordingStart returns a random opaque handle. RecordingFinish consumes it,
waits for device shutdown, downmixes/resamples/gates inside the module, and
returns an output handle. RecordingCancel consumes the recording and waits for
its device thread to finish. Dropping the module stops remaining handles through
the existing library Drop implementation. Invalid processing arguments retain
the recording so the host can cancel it. Device/setup faults release capacity.

ReadAudioOutput accepts at most 256 KiB per call and returns base64 WAV bytes;
ReleaseAudioOutput frees the held output. Unknown, consumed and released handles
fail explicitly. Output size is checked before resampling to bound allocation
and again against the aggregate store budget. No calls occur per audio sample.

The native bridge in device_native.rs touches real devices and follows the
repository's existing device-file coverage exception. Backend fixtures exercise
permission admission, exclusivity, cancellation, preparation, bounded reads,
release and faults without hardware. Native fault detail is for product
presentation only; host telemetry must use the safe reason code without detail.
