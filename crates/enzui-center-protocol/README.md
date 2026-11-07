# enzui-center-protocol

The wire shared by the optional `enzui-center` agent daemon and every app
connector. Both ends build on this crate, so a frame or proof one side writes
is the one the other reads.

| module | what |
|---|---|
| `framing` | `u32` big-endian length-prefixed JSON frames over any `std::io` stream |
| `protocol` | message and credential types |
| `proof` | the HMAC handshake proofs, direction-separated, with fresh nonces from both sides |
| `mac` | the MAC those proofs use |

It holds data and pure functions only. Sockets, peer credentials and the
handshake's I/O belong to each connector; the app-side one is
`enzui-agent`.
