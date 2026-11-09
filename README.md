# spectra

A free spectrum analyzer and waterfall for macOS, built for hunting unknown
signals across a wide span. Rust, egui on wgpu (Metal), no C libraries: HackRF,
RTL-SDR and USRP B2xx are driven by pure-Rust USB drivers through `seify`.

Design: `docs/superpowers/specs/2026-11-07-spectrum-analyzer-design.md`.

## Build and run

Nix manages the toolchain:

```sh
nix develop            # or `direnv allow`
cargo run -p spectra   # opens the last saved layout
cargo run -p spectra -- recording.sigmf-meta
```

Plain `cargo` with a stable Rust toolchain works too.

## Sources

Everything is opened from the **Source** menu:

- **Open SigMF…** replays a `.sigmf-meta` + `.sigmf-data` pair (`cf32_le`,
  `ci16_le`, `ci8`, `cu8`) with pause, loop, seek and speed in the side panel.
- **Radios** found on USB are listed by name. Center, sample rate, gain
  elements, antenna and AGC appear in the side panel as the device describes
  them. One radio can feed several views.
- **Listen for FFT stream** accepts finished spectrum rows from another program
  over TCP; the wire format is in `docs/fft-stream-format.md`.

**View → New view on …** adds a tab. Each tab has its own FFT size, window,
averaging, max-hold, sub-band (digital down-conversion), reference level and
range. Click the spectrum to drop a marker, drag to move it, right-click to
remove, press `P` over a marker to snap it to the nearest peak.

The layout, views and sources are saved on exit to
`~/Library/Application Support/spectra/layout.ron` (or the platform config dir).

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p spectra-sources --example make_sigmf -- /tmp/test 10   # synthetic recording
cargo run -p spectra-sources --example probe                        # list and stream from a radio
cargo run -p spectra-sources --example fft_stream_producer          # feed the FFT-stream input
```
