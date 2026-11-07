# enzui-resources

The **effectful half** of remote resource loading. `enzui-core`'s
`async_image` owns policy and the pure lifecycle reducer; this crate owns the
machinery that actually does work: a bounded worker pool, real transport
cancellation, magic-byte-driven decode, single-flight coalescing of duplicate
requests, and stable reuse of decoded handles.

## The ownership seam

Widgets never spawn work and never perform hidden I/O. The application:

1. constructs and owns an `ImageService`,
2. feeds its `ServiceEvent`s into the core reducer, and
3. renders the reducer snapshot.

That keeps the pure decision logic in `core` testable and confines every side
effect to this crate.

## What to reach for first

- `ImageService` - the owned service an app holds and drives.
- `ServiceEvent` - the events you pump back into the core reducer.

## Transport

The real HTTP(S) implementation is behind the default **`curl-transport`**
feature. It is configured for dynamic system libcurl with its OpenSSL and zlib
development interfaces; the crate does not enable `static-curl` or `static-ssl`.

### Linux build prerequisites (default features)

Install a C toolchain, `pkg-config`, and the development packages for libcurl,
OpenSSL, and zlib before the first build:

```sh
# Debian / Ubuntu
sudo apt-get install build-essential pkg-config libcurl4-openssl-dev libssl-dev zlib1g-dev

# Fedora / RHEL
sudo dnf install gcc pkgconf-pkg-config libcurl-devel openssl-devel zlib-devel

# Arch Linux
sudo pacman -S --needed base-devel pkgconf curl openssl zlib
```

These packages are build-time requirements whenever `curl-transport` is
compiled, including the default workspace and desktop-example builds. Disable default features
to run the service with an **injected transport** only; tests and sandboxed apps
can then use it without compiling or linking libcurl, OpenSSL, or zlib. The
optional `heif` feature is runtime-loaded and does not add a link-time libheif
prerequisite.

## Optional HEIC / HEVC-branded HEIF decode

The `heif` Cargo feature is off by default. When enabled, the decoder loads the
host's `libheif` at runtime; a missing library or HEVC plugin is an honest
`DecodeError::Unsupported`, while malformed HEIC is `DecodeError::DecodeFailed`.
This avoids making every application link native HEVC-facing code, but enabling
it still opts into a C/C++ decoder reached from untrusted files and HEVC's patent
pool/licensing constraints. The crate does not execute desktop thumbnailer
helpers without a real sandbox.

AVIF is not silently enabled: `image`'s `avif-native` decoder adds dav1d and
decodes before exposing dimensions, which cannot currently preserve the
pre-allocation `DecodeLimits` contract. GIF (animation), TIFF and ICO also remain
outside the deliberately short default set.

## Optional first-frame posters

Enable `first-frame-previews` and call `first_frame::decode` or
`first_frame::decode_resized` on an existing owned worker. GIF produces its first
image on the logical canvas; APNG returns its first animation frame, while the
codec decodes and discards a separate default image when present. Later frames
are not decoded or validated. Other stills reuse static decoding and orientation.
Source dimensions must pass `DecodeLimits` before a thumbnail is fitted, even
for small requested bounds. Resampling uses the shared Lanczos path without
enlargement, and retained `rgba_bytes` is exactly width × height × four.

Cancellation is cooperative before/after synchronous codec work. PNG header
storage retains the shared 64 KiB minimum budget; native working-buffer refusals
report `DecodeError::WorkingAllocationLimit`. APNG keeps additional canvases and
can refuse a budget that would hold one final frame. Animated WebP remains
refused because its codec does not enforce allocation limits on animated frame
buffers. The default static and remote-resource paths still refuse animation,
including when this feature is enabled. No playback or background worker is
created by these functions.

## Optional local image providers

Enable `local-images` for [`local_image`](src/local_image.rs). Its default registry
reuses static PNG/JPEG/WebP/BMP decoding and recognizes HEIC; enable `heif` and
install the runtime library/plugin to make that adapter available. Applications
register trusted providers for additional flattened PSD, developed RAW or motion
previews. Those providers remain explicit dependencies; the registry installs
and executes no helpers automatically.

`LocalImages` dispatches discovery and decode through an app-owned bounded pool.
An existing worker can call `ProviderRegistry::decode` directly. Content recognition
selects the provider; availability, corrupt content, unknown formats, policy
refusal and cancellation have distinct errors. Header dimensions are admitted
before provider pixel decode. Returned RGBA storage, target bounds, no enlargement
and aggregate motion frame/duration/byte limits are validated. Providers must
honor the same budgets before allocation; they are trusted code, not a sandbox.
Picker filters derive from `available_formats` on a worker, and extension lists
promise neither camera models nor codec variants. Outputs are orientation-adjusted
8-bit SDR rasters without implicit ICC conversion; layers and sensor data remain
outside this API.

## Optional persistent derived previews

Enable `derived-previews` for [`derived_preview`](src/derived_preview.rs).
`PreviewKey` combines a caller source fingerprint, target bounds, edit recipe and
algorithm/codec version. Capture source identity on a worker; refresh the key
when it changes. The validator rechecks disk hits and derivations. A RAM hit
trusts the current key. Derivers are trusted code and must honor allocation
limits before returning resolved RGBA; pass their supplied cancellation callback
into local providers, document renderers or other cooperative decoders.

`DerivedPreviews` owns bounded waiting/running admission over an existing
`BlockingTaskDispatcher`. Requests coalesce by key; `hints` retains up to 64
visible and 64 recent keys and prioritizes them at dispatch. Hints never enqueue
work. Drive `dispatch` and `complete` from the app runtime; no update method reads
files. The caller keeps the pool owner. `PreviewStore` exposes the same synchronous
storage path to already-owned workers. Disk usage, clear and budget changes have
worker tasks; await their successful settlement before reporting completion.

The root belongs exclusively to this cache format. Entries are atomically staged
relative to a retained private directory. Cooperating instances serialize disk
operations under a stable advisory lock; decoders run outside it and may duplicate
a miss across processes. A persistent epoch blocks pre-clear/pre-budget-change
work from publication. The first storage operation initializes the root-wide disk
budget; later opens respect it until `set_disk_budget` changes it. Visibility
protection applies to the hints supplied for each operation, not another process's
unpublished viewport. LRU clocks persist across restarts. PNG retains alpha and
omits source metadata. Storage failures accompany usable pixels as `cache_error`.

Disk usage conservatively counts entry headers and allocation rounded to 4 KiB,
or observed allocated blocks if larger, plus root/lock/epoch overhead and an 8 KiB
coordination/staging reserve. Entries are pruned before staging and after commit.
A budget smaller than coordination overhead cannot retain previews. Zero prunes
entries and keeps the small coordination files required by concurrent instances;
usage continues to report them. Decoded RAM is separately limited by
`decoded_ram_bytes`. Exact budgets, zoom resolution, edit identities, defaults
and settings UI remain application-owned.

## What this crate is **not** for

- Not policy - *whether* to load, retry, or evict is decided by the core reducer,
  not here.
- Not a general HTTP client - it is scoped to the framework's image lifecycle.
