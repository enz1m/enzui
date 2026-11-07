# Bounded native media source

This is an altered MIT-licensed subset of GStreamer's Curl HTTP source from
1.28.5, commit `727ceb91886862d200f423baf36cde2bb7ce5b4d`. `UPSTREAM.json`
records the exact upstream input hashes; source notices and `LICENSE-MIT`
retain British Broadcasting Corporation copyright and Sam Hurst's authorship.
The LGPL-only registration header is omitted. The private registration and Rust
adapter belong to enzUI. No system plugin or global HTTP factory rank is changed.

The `gstreamer` feature of `enzui-services` compiles the source into its own
binary. Other feature sets do not probe native media packages. The build uses
GStreamer 1.14 or newer development headers and libcurl 7.68 or newer; Cargo places compiler
outputs in its assigned target. No runtime plugin installation is required.

HTTPS playback selects `enzuicurlhttpsrc` through a private URI scheme, then
checks the original approved HTTPS location, source identity and fixed receive
limit. Ordinary HTTP retains the Soup path unless disk read-ahead is enabled.
Disk read-ahead selects the same bounded Curl producer for HTTP and HTTPS. Destination approval, authenticated
peer-pinning proxy, strict TLS and disabled redirects still apply.

The default native body buffer admits at most 256 KiB. A full buffer returns
`CURL_WRITEFUNC_PAUSE` without consuming the pending chunk. Only the owning Curl
multi task resumes the easy handle after downstream consumption. Poll wakeups
cover consumption and cancellation. Each source owns its multi task, preventing
cross-instance lifecycle races. Every demand seek, including a repeated Range start, discards the old easy handle,
pending bytes and pause flags before admitting the new range. HTTP/1.1 avoids
multiplexed HTTP/2 receive buffering. Proxy bypass environment variables cannot
turn the approved proxy off. Native Curl verbose callbacks are omitted so they
cannot dump authorization headers or media bodies. Header callbacks receive
terminated copies before parsing. Task startup failures propagate to the pipeline,
and a configured retry count of zero remains zero.

Read-only `receive-buffer-limit`, `buffered-bytes`, `peak-buffered-bytes` and
`backpressure-count`, plus `receive-paused`, expose native body-byte and pause observations.
The body-buffer bound does not include kernel socket buffers, TLS state, Curl
metadata or downstream decoder buffers.

Carry the receive callback, owner-thread resume, wakeups and request reset
behavior together. Rebase risk is medium: preserve Curl callback consumption
semantics and the multi-context/buffer lock order. Remove this subset when a
system source provides equivalent bounded backpressure, cancellation and peer
policy with the same real TLS pause/resume/seek regression checks. The separate
Soup CONNECT authentication problem remains open.


Opt-in disk read-ahead imports a duplicated close-on-exec descriptor for a
private, anonymous, read/write regular file of exactly the configured capacity.
The Rust owner fully reserves it before adoption. A bounded unread FIFO uses
explicit read/write cursors and queued length; consumed bytes are discarded,
with no historical-range cache. File I/O handles wraparound and interrupted
syscalls under the buffer mutex. Full storage pauses Curl without consuming its
pending callback. Each emitted GstBuffer remains at most 256 KiB. Seek resets
queued bytes and cursors without truncating the reservation. Partial setup and
file I/O failures stop the stream; there is no unreserved or memory fallback.
The last descriptor close reclaims the anonymous inode, including on a crash.

Startup-only `read-ahead-capacity` and borrowed `read-ahead-fd` configure this
mode. Read-only `read-ahead-ready`, `read-ahead-buffered-bytes`,
`read-ahead-peak-bytes` and `read-ahead-file-bytes` report its actual state.
In disk mode the original body-memory metrics are zero; they exclude emitted
GstBuffers and the OS page cache. Kernel, TLS, Curl and decoder allocations are
separate. The source advertises bandwidth-limited scheduling for its private URI aliases,
so native source bins activate network buffering. Disk mode disables playbin's
history ring and leaves DOWNLOAD off;
queue2 disk/history wrapping is not used.
