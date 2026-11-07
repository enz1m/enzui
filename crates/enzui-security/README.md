# enzui-security

Small, dependency-light security and filesystem primitives shared across the
enzUI crates, so each rule has one implementation.

| module | what |
|---|---|
| `secret` | open a credential file once, without following symlinks, check owner, type and mode on the descriptor, and hold the bytes in a `Secret` that wipes on drop and redacts itself |
| `secure_dir` | private store roots and directory capabilities retained by descriptor, so a swapped path cannot redirect later operations |
| `tempstage` | the staged temp file behind every atomic write: create, write, sync, then publish by descriptor-relative rename |
| `durable` | the one parent-directory durability barrier |
| `quarantine` | move a file aside, without clobbering, into an unpredictable sibling name |
| `runtime_dir` | the per-user 0700 directory for sockets and other runtime objects |
| `random` | the single OS-backed random-byte source for security identities |
| `cancellation` | one cancellation token with exactly one terminal state |
| `url_policy` | URL and transport safety policy for remote HTTP resources |
| `redirect_capture` | which navigation is a sign-in callback for the host rather than a page |
| `loopback_proxy` | capability authentication for loopback HTTP proxies |
| `decode_limits` | limits and typed verdicts for encoded image size and decoded dimensions |

Filesystem helpers retain validated descriptors for operations inside their
owned stores. URL, cancellation and decode policies also provide pure checks
that do not acquire filesystem authority.

## Features

Both features are for tests only and off by default.

- `fault-injection` adds fault hooks to the durability, staging and quarantine
  paths. Without it every seam compiles to the plain syscall.
- `secret-observer` reports a secret's allocation identity and post-wipe byte
  count, so a test can check the wipe without reading freed memory.
