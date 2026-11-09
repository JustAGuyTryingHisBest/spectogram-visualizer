# FFT-stream input format

`spectra` can display spectrum rows produced by another program. The producer
connects over TCP (default port 4590, chosen in the Source menu) and sends one
header followed by frames until it disconnects. All integers and floats are
little-endian. Geometry and units are fixed for the life of a connection; a
producer that needs to change them disconnects and reconnects.

## Header (once, 28 bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | `u8[4]` | magic, ASCII `SPEC` |
| 4 | `u16` | version, `1` |
| 6 | `u8` | units: `0` = dBFS, `1` = dBm |
| 7 | `u8` | reserved, `0` |
| 8 | `f64` | center frequency, Hz |
| 16 | `f64` | span, Hz |
| 24 | `u32` | bin count (1 to 1,048,576) |

## Frame (repeated, 8 + 4 × bin count bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | `u64` | timestamp, nanoseconds (producer-defined epoch) |
| 8 | `f32[bin count]` | power per bin in the header's units, lowest frequency first |

Bin `i` is centered at `center − span/2 + (i + 0.5) × span / bin_count`.
A bad magic, unsupported version, out-of-range bin count or short frame closes
the connection; `spectra` reports the reason in its status bar and accepts the
next connection.

## Example producer

```sh
cargo run -p spectra-sources --example fft_stream_producer -- 127.0.0.1:4590
```
