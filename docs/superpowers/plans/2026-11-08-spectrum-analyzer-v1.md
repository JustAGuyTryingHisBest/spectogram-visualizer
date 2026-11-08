# Spectrum Analyzer v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust/egui spectrum analyzer for macOS that shows live spectrum and waterfall views from HackRF, USRP B205-mini, RTL-SDR, SigMF replay, or a network FFT stream, with multiple tiled views, max-hold/average traces, and markers.

**Architecture:** Three layers joined by channels. Sources (one per device/file) fan IQ blocks or spectrum rows out to subscribers. One processing chain per view runs gapless on a worker thread (optional sub-band DDC, window, FFT, dB, averaging, max-hold) and emits rows. The egui UI thread only consumes rows and draws: spectrum as a line strip, waterfall as a GPU texture updated one row at a time.

**Tech Stack:** Rust 2024 edition, stable toolchain ≥ 1.85 (nixpkgs has 1.99). `eframe`/`egui` 0.36.2 (wgpu backend, default features), `egui_dock` 0.21.1, `rustfft` 6.4, `num-complex` 0.4, `crossbeam-channel` 0.5, `seify` 0.26.2 with features `hackrf,rtlsdr,uhd` and `default-features = false` (pure-Rust `nusb` drivers; no libusb, libhackrf or libuhd needed), `serde`/`serde_json` 1, `ron` for layout persistence. Nix flake for the dev shell.

**Spec:** `docs/superpowers/specs/2026-11-07-spectrum-analyzer-design.md`

## Global Constraints

- Chains are gapless: every IQ sample from the source is windowed and transformed. Rows may be combined or dropped only in the view. (spec: Processing chains)
- Nothing upstream waits on anything downstream: bounded channels, drop-oldest, drop counters. (spec: Threading)
- The device reader thread does nothing but hand blocks to subscribers. (spec: Threading)
- Device-specific concepts never appear in `dsp` or `app` crates. Sources expose tunables as a typed parameter list. (spec: Sources / Anti-goals)
- Rows keep a per-view ring buffer even though nothing scrubs it yet. (spec: Design for later)
- Defaults: window Blackman-Harris, FFT 2048, averaging 8, units dBFS, colormap inferno. (spec: Calls I made)
- FFT-stream geometry and units are fixed per connection, sent once in a header. (spec: Calls I made)
- Nix manages dependencies: the flake's dev shell provides the toolchain and any system libraries, and all build/test commands run inside `nix develop`. (owner decision 2026-11-08)
- Variable names are descriptive; no single-letter names outside closures and loop indices (user's global rule).

## Review Focus

1. A SigMF file whose data length is not a whole number of samples, or whose `core:datatype` is unsupported: open must fail with a clear error, never panic or read garbage. Test added to Task 4.
2. Source sample rate changes while views are attached: every chain on that source must rebuild its FFT span and the waterfall texture must not display stale frequency axis labels. Test added to Task 10.
3. A chain on a slow machine falls behind its source: the UI must show an overload indicator and dropped-block count, and the device reader must keep running. Test added to Task 5.
4. An FFT-stream producer disconnects mid-frame or sends a bin count that does not match the header: the source must close that connection, report the error, and accept the next connection. Test added to Task 11.
5. A device disappears (USB unplug) while streaming: the view keeps its last rows, the status bar says the source stopped, and the app does not exit. Test added to Task 9.

---

## File structure

```
Cargo.toml                      workspace
flake.nix                       dev shell: rustc, cargo, rust-analyzer, clippy, rustfmt
rust-toolchain.toml             stable
crates/core/                    spectra-core: shared types, source traits, Broadcast fan-out, params
  src/lib.rs
  src/types.rs                  StreamInfo, IqBlock, SpectrumRow, PowerUnits
  src/broadcast.rs              Broadcast<T>, Subscriber<T> with drop counting
  src/params.rs                 ParamSpec, ParamKind, ParamValue
  src/source.rs                 IqSource, SpectrumSource traits, SourceHandle
crates/dsp/                     spectra-dsp: window, fft, averaging, max-hold, sub-band DDC, Chain, worker
  src/lib.rs
  src/window.rs
  src/chain.rs                  Chain (pure, synchronous)
  src/subband.rs                mixer + FIR decimator
  src/worker.rs                 ChainWorker thread: Subscriber<IqBlock> -> rows channel
crates/sources/                 spectra-sources: concrete sources
  src/lib.rs
  src/sigmf.rs                  SigMF replay (own serde structs; no sigmf crate)
  src/seify_source.rs           any seify device (HackRF, RTL-SDR, USRP)
  src/fft_stream.rs             TCP spectrum-row input
  examples/fft_stream_producer.rs  synthetic producer for tests and docs
crates/app/                     spectra: the binary
  src/main.rs
  src/app.rs                    eframe::App, dock tree, source manager wiring
  src/view.rs                   View: chain config, row ring, traces, markers
  src/spectrum_plot.rs          line-strip spectrum with axes and markers
  src/waterfall.rs              texture ring + colormap
  src/colormap.rs               inferno LUT
  src/source_panel.rs           per-source param UI from ParamSpec
  src/layout.rs                 ron persistence of dock tree + view configs
docs/fft-stream-format.md       wire format for third-party producers
```

---

### Task 1: Workspace, flake, core types and source traits

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `flake.nix`, `.envrc`
- Create: `crates/core/Cargo.toml`, `crates/core/src/{lib,types,params,source}.rs`
- Test: `crates/core/src/types.rs` (unit tests inline)

**Interfaces:**
- Produces:
  ```rust
  // types.rs
  pub struct StreamInfo { pub name: String, pub center_hz: f64, pub sample_rate_hz: f64, pub calibration_offset_db: Option<f32> }
  pub struct IqBlock { pub info: StreamInfo, pub samples: Arc<[Complex32]>, pub sequence: u64, pub received_at: Instant }
  #[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)] pub enum PowerUnits { Dbfs, Dbm }
  pub struct SpectrumRow { pub center_hz: f64, pub span_hz: f64, pub units: PowerUnits, pub power_db: Arc<[f32]>, pub received_at: Instant }
  impl SpectrumRow { pub fn bin_hz(&self, bin_index: usize) -> f64; pub fn bin_width_hz(&self) -> f64 }
  // params.rs
  pub enum ParamKind { Range { min: f64, max: f64, step: f64, unit: &'static str }, Toggle, Choice(Vec<String>) }
  pub struct ParamSpec { pub key: String, pub label: String, pub kind: ParamKind }
  pub enum ParamValue { Number(f64), Toggle(bool), Choice(String) }
  // source.rs
  pub trait IqSource: Send {
      fn info(&self) -> StreamInfo;
      fn subscribe(&self) -> Subscriber<IqBlock>;
      fn params(&self) -> Vec<ParamSpec>;
      fn param(&self, key: &str) -> Option<ParamValue>;
      fn set_param(&mut self, key: &str, value: ParamValue) -> anyhow::Result<()>;
      fn status(&self) -> SourceStatus;
  }
  pub trait SpectrumSource: Send { fn info(&self) -> StreamInfo; fn subscribe(&self) -> Subscriber<SpectrumRow>; fn status(&self) -> SourceStatus; }
  #[derive(Clone, Debug, PartialEq)] pub enum SourceStatus { Streaming, Paused, Stopped(String) }
  pub enum SourceHandle { Iq(Box<dyn IqSource>), Spectrum(Box<dyn SpectrumSource>) }
  ```
  Reserved param keys every IqSource must honor when it can: `"center_hz"`, `"sample_rate_hz"`, `"gain_db"`. `Subscriber` comes from Task 2; declare the traits in Task 1 with `Subscriber` as a placeholder `pub struct Subscriber<T>(pub crossbeam_channel::Receiver<T>)` that Task 2 replaces.

- [ ] **Step 1: Write workspace files.** `Cargo.toml` with `[workspace] members = ["crates/*"]`, `resolver = "3"`, shared `[workspace.dependencies]` pinning the versions in Tech Stack. `rust-toolchain.toml` channel `stable`. `flake.nix` with `devShells.default` from nixpkgs containing `rustc cargo clippy rustfmt rust-analyzer pkg-config` (follow nixpkgs `mkShell` conventions; no overlays). `.envrc` with `use flake`.
- [ ] **Step 2: Write failing test** in `types.rs`: `bin_hz_centers_span` asserts for a row with `center_hz = 915e6`, `span_hz = 20e6`, 2048 bins: `bin_width_hz() == 20e6/2048.0` and `bin_hz(1024) == 915e6` (bin index `n/2` is the center, matching FFT-shifted order).
- [ ] **Step 3: Run** `cargo test -p spectra-core` → FAIL (missing items).
- [ ] **Step 4: Implement** the types, params, and traits above. `bin_hz(i) = center_hz + (i as f64 - n as f64 / 2.0) * bin_width_hz()` so bin `n/2` is exactly the center.
- [ ] **Step 5: Run** `cargo test -p spectra-core` → PASS. `cargo clippy --workspace -- -D warnings` → clean.
- [ ] **Step 6: Commit** `feat(core): workspace, shared types and source traits`.

---

### Task 2: Broadcast fan-out with drop-oldest and drop counting

**Files:**
- Create: `crates/core/src/broadcast.rs`
- Modify: `crates/core/src/source.rs` (use real `Subscriber`)
- Test: inline

**Interfaces:**
- Produces:
  ```rust
  pub struct Broadcast<T: Clone + Send> { .. }
  impl<T: Clone + Send> Broadcast<T> {
      pub fn new() -> Self;
      pub fn subscribe(&self, capacity: usize) -> Subscriber<T>;
      pub fn publish(&self, item: T);          // never blocks; drops oldest per full subscriber
      pub fn subscriber_count(&self) -> usize;  // dropped subscribers are pruned on publish
  }
  pub struct Subscriber<T> { .. }
  impl<T> Subscriber<T> {
      pub fn recv(&self) -> Option<T>;                  // None when broadcast dropped
      pub fn recv_timeout(&self, d: Duration) -> Option<T>;
      pub fn try_recv(&self) -> Option<T>;
      pub fn dropped(&self) -> u64;                      // items this subscriber lost
  }
  ```

- [ ] **Step 1: Write failing tests:** `slow_subscriber_drops_oldest_and_counts` (capacity 2, publish 1,2,3,4 → recv yields 3,4; `dropped() == 2`); `publish_never_blocks_on_full` (publish 1000 items into capacity 1 completes in < 100 ms); `dropped_subscriber_is_pruned` (subscribe, drop, publish → `subscriber_count() == 0`).
- [ ] **Step 2: Run** `cargo test -p spectra-core broadcast` → FAIL.
- [ ] **Step 3: Implement** with `crossbeam_channel::bounded`; on `TrySendError::Full`, `try_recv` one item, increment an `Arc<AtomicU64>` shared with the `Subscriber`, retry once. Subscribers in a `Mutex<Vec<...>>`; prune on `Disconnected`.
- [ ] **Step 4: Run** tests → PASS.
- [ ] **Step 5: Commit** `feat(core): broadcast fan-out with drop-oldest`.

---

### Task 3: Gapless DSP chain: window, FFT, dB, averaging, max-hold

**Files:**
- Create: `crates/dsp/Cargo.toml`, `crates/dsp/src/{lib,window,chain}.rs`
- Test: inline

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
  pub enum Window { BlackmanHarris, Hann, Hamming, Rectangular, FlatTop }
  impl Window { pub fn coefficients(self, length: usize) -> Vec<f32>; pub const ALL: [Window; 5]; pub fn label(self) -> &'static str }
  #[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
  pub struct ChainConfig { pub fft_size: usize, pub window: Window, pub averaging: usize, pub max_hold: bool, pub sub_band: Option<SubBand> }
  impl Default for ChainConfig  // 2048, BlackmanHarris, 8, true, None
  pub struct ChainOutput { pub live_db: Arc<[f32]>, pub average_db: Arc<[f32]>, pub max_hold_db: Option<Arc<[f32]>> }
  pub struct Chain { .. }
  impl Chain {
      pub fn new(config: ChainConfig) -> Self;
      pub fn config(&self) -> &ChainConfig;
      pub fn reset_max_hold(&mut self);
      /// Consumes every sample; calls `emit` once per completed FFT frame, in FFT-shifted order (DC at index n/2).
      pub fn push(&mut self, samples: &[Complex32], emit: &mut dyn FnMut(ChainOutput));
  }
  ```
  `SubBand` is defined in Task 6; in this task declare `pub struct SubBand { pub offset_hz: f64, pub decimation: u32 }` and leave `sub_band` unused.

- [ ] **Step 1: Write failing tests:**
  - `window_coefficients_peak_is_one`: for each `Window::ALL`, `coefficients(1024)` max within 1e-6 of 1.0 and length 1024.
  - `tone_lands_in_expected_bin`: 4096 samples of a unit-amplitude complex tone at +fs/8 (`exp(j 2π (fs/8)/fs n)`), `fft_size 1024`, Rectangular, averaging 1 → every emitted `live_db` argmax is `512 + 128`, and that value is within 0.5 dB of 0.0 dBFS (normalize so a full-scale tone with rectangular window reads 0 dBFS).
  - `chain_is_gapless`: push 1000 samples then 1048 samples with `fft_size 1024` → exactly 2 emits total (no sample lost across the boundary, 2048 samples = 2 frames).
  - `average_and_max_hold_behave`: feed a tone then silence (−∞ guarded to −200 dB floor); after silence frames, `max_hold_db` keeps the tone peak, `average_db` decays toward the floor; `reset_max_hold` then one silent frame gives `max_hold_db == live_db`.
- [ ] **Step 2: Run** `cargo test -p spectra-dsp` → FAIL.
- [ ] **Step 3: Implement.** Window formulas: standard 4-term Blackman-Harris, Hann, Hamming, flat-top (5-term, Matlab coefficients), all normalized so peak = 1. Power: `10*log10(|X|^2 / (sum(window)^2))` with a floor of `-200.0`. Averaging is an exponential moving average `avg += (live - avg) / averaging`, seeded with the first frame. `rustfft::FftPlanner<f32>` plan cached in the struct; keep a `Vec<Complex32>` carry buffer for leftover samples.
- [ ] **Step 4: Run** tests → PASS.
- [ ] **Step 5: Commit** `feat(dsp): gapless window/FFT chain with averaging and max-hold`.

---

### Task 4: SigMF replay source

**Files:**
- Create: `crates/sources/Cargo.toml`, `crates/sources/src/{lib,sigmf}.rs`
- Test: `crates/sources/tests/sigmf_replay.rs` (writes temp files)

**Interfaces:**
- Consumes: `IqSource`, `Broadcast`, `IqBlock`, `StreamInfo`, params (Task 1–2).
- Produces:
  ```rust
  pub struct SigmfSource { .. }
  impl SigmfSource {
      /// `path` is the `.sigmf-meta` file; the `.sigmf-data` sibling is required.
      pub fn open(path: &Path) -> anyhow::Result<Self>;
      pub fn total_samples(&self) -> u64;
      pub fn position(&self) -> u64;            // current sample index
      pub fn seek(&self, sample_index: u64);
      pub fn set_paused(&self, paused: bool);
      pub fn set_loop(&self, looping: bool);
      pub fn set_speed(&self, multiplier: f64);  // 1.0 = real time
  }
  impl IqSource for SigmfSource  // params(): "paused" Toggle, "loop" Toggle, "speed" Range 0.1..=8.0 step 0.1
  ```
  Supported `core:datatype`: `cf32_le`, `ci16_le`, `ci8`, `cu8`. Block size 16384 samples, paced with `std::thread::sleep` to `sample_rate / speed`. Center frequency from `captures[0]["core:frequency"]`, else 0.0. Own serde structs for global/captures; do not add the `sigmf` crate.

- [ ] **Step 1: Write failing tests** (each writes a meta+data pair into `tempfile::tempdir()`):
  - `plays_cf32_at_requested_rate`: 65536 samples at `core:sample_rate 1e6`, speed 8.0 → first 4 blocks arrive within 150 ms, total samples received == 65536, `info().center_hz == 915e6`, `info().sample_rate_hz == 1e6`.
  - `rejects_truncated_data`: data file of 7 bytes with `cf32_le` → `open` returns `Err` whose message contains `"not a whole number of samples"`.
  - `rejects_unknown_datatype`: `core:datatype = "cf64_le"` → `Err` containing `"unsupported datatype"`.
  - `loop_wraps_and_seek_moves`: 4096 samples, loop on, speed 8 → receives more than 4096 samples within 1 s; `seek(0)` then `position() < 4096`.
- [ ] **Step 2: Run** `cargo test -p spectra-sources sigmf` → FAIL.
- [ ] **Step 3: Implement.** Reader thread with `Arc<SharedState>` (`AtomicU64` position, `AtomicBool` paused/loop, `Mutex<f64>` speed). Conversion: `ci16` divide by 32768, `ci8` by 128, `cu8` `(v as f32 - 127.5)/127.5`. At end of file without loop → `SourceStatus::Stopped("end of file")`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(sources): SigMF replay source with pause/seek/loop/speed`.

---

### Task 5: Chain worker thread

**Files:**
- Create: `crates/dsp/src/worker.rs`
- Test: inline (uses `Broadcast<IqBlock>` directly)

**Interfaces:**
- Produces:
  ```rust
  pub struct RowFrame { pub row: SpectrumRow, pub average_db: Arc<[f32]>, pub max_hold_db: Option<Arc<[f32]>>, pub source_dropped_blocks: u64 }
  pub struct ChainWorker { .. }
  impl ChainWorker {
      pub fn spawn(subscriber: Subscriber<IqBlock>, config: ChainConfig, row_capacity: usize) -> Self;
      pub fn rows(&self) -> &Subscriber<RowFrame>;          // UI side; drop-oldest
      pub fn set_config(&self, config: ChainConfig);       // rebuilds Chain on the worker
      pub fn reset_max_hold(&self);
      pub fn stats(&self) -> WorkerStats;
  }
  #[derive(Clone, Copy, Debug, Default)] pub struct WorkerStats { pub rows_per_second: f32, pub input_dropped_blocks: u64, pub overloaded: bool }
  ```
  `overloaded` is true when `input_dropped_blocks` increased within the last second. Row `center_hz`/`span_hz` come from the block's `StreamInfo` (or the sub-band, Task 6). `units` is `Dbm` when `calibration_offset_db` is `Some` (added to every bin), else `Dbfs`.

- [ ] **Step 1: Write failing tests:** `worker_emits_rows_with_source_geometry` (publish 4096-sample blocks with center 100e6 rate 2e6, fft 1024 → rows with `center_hz 100e6`, `span_hz 2e6`, 1024 bins); `overload_is_reported_when_input_drops` (subscriber capacity 1, publish 50 blocks faster than a sleeping fake chain can drain → `stats().input_dropped_blocks > 0` and `overloaded`); `calibration_offset_switches_units_to_dbm`.
- [ ] **Step 2: Run** `cargo test -p spectra-dsp worker` → FAIL.
- [ ] **Step 3: Implement.** Config changes via a `crossbeam_channel` control channel polled between blocks. Thread exits when the input subscriber returns `None`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(dsp): chain worker thread with overload stats`.

---

### Task 6: Sub-band selection (DDC + decimation)

**Files:**
- Create: `crates/dsp/src/subband.rs`
- Modify: `crates/dsp/src/chain.rs` (apply sub-band before windowing), `crates/dsp/src/worker.rs` (row geometry)
- Test: inline

**Interfaces:**
- Produces:
  ```rust
  pub struct SubBand { pub offset_hz: f64, pub decimation: u32 }   // decimation ∈ {2,4,8,16,32,64}
  pub struct Ddc { .. }
  impl Ddc {
      pub fn new(sub_band: SubBand, input_rate_hz: f64) -> Self;
      pub fn output_rate_hz(&self) -> f64;
      /// Appends decimated samples to `out`; keeps phase and filter state across calls.
      pub fn process(&mut self, input: &[Complex32], out: &mut Vec<Complex32>);
  }
  ```
  Row geometry with a sub-band: `center_hz = source.center_hz + offset_hz`, `span_hz = sample_rate / decimation`.

- [ ] **Step 1: Write failing tests:** `ddc_moves_tone_to_baseband` (tone at +250 kHz in a 2 MS/s stream, `offset_hz 250e3`, decimation 4 → output tone within 1 bin of DC in a 1024-point FFT and output length == input/4 ± filter delay); `ddc_rejects_out_of_band_tone` (tone at +900 kHz with same settings is attenuated by ≥ 40 dB relative to in-band).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** Complex NCO mixer by `exp(-j 2π offset/fs n)` with phase carried between calls; windowed-sinc (Blackman) low-pass FIR with cutoff `0.4 * fs/decimation`, `8 * decimation + 1` taps, polyphase-free direct form computing only kept outputs.
- [ ] **Step 4: Run** → PASS; also rerun Task 3 and 5 tests.
- [ ] **Step 5: Commit** `feat(dsp): sub-band DDC and decimation`.

---

### Task 7: App skeleton with one spectrum view from SigMF

**Files:**
- Create: `crates/app/Cargo.toml`, `crates/app/src/{main,app,view,spectrum_plot,colormap}.rs`
- Test: `crates/app/src/spectrum_plot.rs` inline for coordinate mapping; manual run

**Interfaces:**
- Consumes: `SigmfSource`, `ChainWorker`, `RowFrame`.
- Produces:
  ```rust
  pub struct View { pub id: ViewId, pub source_id: SourceId, pub config: ChainConfig, pub ref_level_db: f32, pub range_db: f32, pub show_live: bool, pub show_average: bool, pub show_max_hold: bool, pub markers: Vec<Marker>, pub mode: ViewMode, .. }
  pub enum ViewMode { Spectrum, Waterfall, Both }
  pub struct RowRing { pub fn new(capacity: usize) -> Self; pub fn push(&mut self, frame: RowFrame); pub fn latest(&self) -> Option<&RowFrame>; pub fn len(&self) -> usize }   // capacity 4096 rows
  pub struct PlotMapping { .. }  // rect + freq/dB ranges
  impl PlotMapping { pub fn x_for_hz(&self, hz: f64) -> f32; pub fn hz_for_x(&self, x: f32) -> f64; pub fn y_for_db(&self, db: f32) -> f32 }
  pub fn draw_spectrum(ui: &mut egui::Ui, view: &mut View, frame: Option<&RowFrame>) -> egui::Response;
  ```
  CLI: `spectra [path.sigmf-meta]` opens that file as the first source. Spectrum colors: live `#8fb6ff`, average `#6fd49a`, max-hold `#ff8c42`. Grid every 10 dB and 10 vertical divisions, labels in MHz with 3 decimals.

- [ ] **Step 1: Write failing test** `mapping_round_trips`: `PlotMapping` for rect (0,0)-(800,200), 905–925 MHz, −110..−40 dB: `x_for_hz(915e6) == 400.0`, `hz_for_x(x_for_hz(910.5e6))` within 1 Hz, `y_for_db(-40.0) == 0.0`, `y_for_db(-110.0) == 200.0`.
- [ ] **Step 2: Run** `cargo test -p spectra` → FAIL.
- [ ] **Step 3: Implement** mapping, `RowRing`, `View`, `draw_spectrum` (uses `ui.painter().add(Shape::line(points, stroke))` per trace; a downsample to one min/max pair per pixel column when bins > width), `App` that owns one `SigmfSource`, one `ChainWorker`, one `View`, drains `rows()` each frame with `try_recv` into the ring, and calls `ctx.request_repaint()`. `main.rs` uses `eframe::run_native` with `NativeOptions` default (wgpu).
- [ ] **Step 4: Run** `cargo test -p spectra` → PASS. Generate a test file with a small script (`crates/sources/examples/make_sigmf.rs`: two tones plus noise, 2 MS/s, 10 s, cf32_le) and run `cargo run -p spectra -- /tmp/test.sigmf-meta`. Expected: a window with a moving spectrum and two peaks; title bar shows fps ≥ 60 in the status text.
- [ ] **Step 5: Commit** `feat(app): eframe skeleton with spectrum plot from SigMF replay`.

---

### Task 8: Waterfall texture and colormap

**Files:**
- Create: `crates/app/src/waterfall.rs`, `crates/app/src/colormap.rs`
- Modify: `crates/app/src/view.rs` (draw Both/Waterfall), `crates/app/src/app.rs`
- Test: inline

**Interfaces:**
- Produces:
  ```rust
  pub struct Colormap { pub fn inferno() -> Self; pub fn lookup(&self, normalized: f32) -> egui::Color32 }  // 256-entry LUT, clamps
  pub struct Waterfall { .. }
  impl Waterfall {
      pub fn new(width_bins: usize, height_rows: usize) -> Self;   // height 1024
      pub fn push_row(&mut self, power_db: &[f32], ref_level_db: f32, range_db: f32, colormap: &Colormap);  // resamples to width_bins
      pub fn ensure_texture(&mut self, ctx: &egui::Context) -> egui::TextureId;
      pub fn draw(&mut self, ui: &mut egui::Ui, rect: egui::Rect);  // newest row at top
      pub fn newest_index(&self) -> usize;   // texture row holding the newest data
      pub fn rows_pushed(&self) -> u64;
  }
  ```
  Implementation: `egui::TextureHandle` of `ColorImage` width×height, `TextureOptions::NEAREST`; `set_partial([0, write_row], one_row_image, ..)` per pushed row; draw with `Image::uv` split into two quads so the ring wraps with newest at top. Width is the view's pixel width rounded to 2048 max; row resampling by max over covered bins.

- [ ] **Step 1: Write failing tests:** `colormap_clamps_and_is_monotonic_in_luminance` (lookup(−1) == lookup(0), lookup(2) == lookup(1), luminance of 0.0 < 0.5 < 1.0); `waterfall_ring_wraps` (height 4, push 6 rows → `newest_index()` correct and `rows_pushed() == 6`).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** Inferno LUT: embed 256 RGB triplets (matplotlib's data) in `colormap.rs`.
- [ ] **Step 4: Run** tests → PASS. Manual: `cargo run -p spectra -- /tmp/test.sigmf-meta` shows waterfall scrolling under the spectrum, two vertical bands, 60 fps.
- [ ] **Step 5: Commit** `feat(app): GPU waterfall with inferno colormap`.

---

### Task 9: Hardware source via seify and the per-source panel

**Files:**
- Create: `crates/sources/src/seify_source.rs`, `crates/app/src/source_panel.rs`
- Modify: `crates/sources/Cargo.toml` (seify features `hackrf,rtlsdr,uhd`, no default), `crates/app/src/app.rs` (source manager: list, open, close)
- Test: inline unit test for param building using `seify` feature `dummy`; manual hardware test

**Interfaces:**
- Produces:
  ```rust
  pub struct SeifySource { .. }
  impl SeifySource {
      pub fn enumerate() -> Vec<DeviceSummary>;   // via seify::enumerate()
      pub fn open(args: &seify::Args, initial_center_hz: f64, initial_sample_rate_hz: f64) -> anyhow::Result<Self>;
  }
  pub struct DeviceSummary { pub label: String, pub args: seify::Args }
  impl IqSource for SeifySource
  // params(): "center_hz" Range from frequency_range(), "sample_rate_hz" Range from get_sample_rate_range(), "gain_db" Range from the device gain range,
  //           one "gain:<element>" Range per gain_elements() entry, "antenna" Choice(antennas()), "agc" Toggle when supported.
  pub struct SourceManager { pub fn open_sigmf(&mut self, path: &Path) -> anyhow::Result<SourceId>; pub fn open_device(&mut self, summary: &DeviceSummary) -> anyhow::Result<SourceId>; pub fn close(&mut self, id: SourceId); pub fn get(&self, id: SourceId) -> Option<&SourceHandle>; pub fn get_mut(..); pub fn ids(&self) -> Vec<SourceId> }
  pub fn draw_source_panel(ui: &mut egui::Ui, source: &mut dyn IqSource);   // renders each ParamSpec: DragValue for Range (SI formatting for Hz), Checkbox, ComboBox
  ```
  Reader thread: `rx.read(&mut [&mut buf], timeout_us 200_000)` into a 16384-sample buffer, publish `IqBlock`, loop until stop flag. On `Err` other than timeout → `SourceStatus::Stopped(err.to_string())` and thread exits. `set_param("center_hz")` calls `rx0.frequency().set` and updates the `StreamInfo` stamped on subsequent blocks; `sample_rate_hz` changes deactivate, set, reactivate the streamer.

- [ ] **Step 1: Write failing test** (feature `dummy` in dev-deps): `dummy_device_exposes_core_params` — open `driver=dummy`, `params()` contains keys `center_hz`, `sample_rate_hz`, `gain_db`; `set_param("center_hz", Number(433e6))` → `info().center_hz == 433e6`.
- [ ] **Step 2: Run** `cargo test -p spectra-sources seify` → FAIL.
- [ ] **Step 3: Implement** source and panel, plus a toolbar "Source ▾" menu in `app.rs` listing `enumerate()` results and "Open SigMF…" (use `rfd` 0.15 for the file dialog).
- [ ] **Step 4: Run** → PASS. Manual with HackRF plugged in: open from menu, tune to 915 MHz / 20 MS/s, see spectrum; unplug → status bar shows `stopped: <error>`, app stays up, last rows remain (Review Focus 5). Repeat with the B205-mini (uhd-rs will load firmware on first open; expect several seconds).
- [ ] **Step 5: Commit** `feat(sources): seify hardware source and per-source parameter panel`.

---

### Task 10: Multiple views with egui_dock and per-view chain settings

**Files:**
- Create: `crates/app/src/view_toolbar.rs`
- Modify: `crates/app/src/app.rs` (DockState<ViewId>, TabViewer impl), `crates/app/src/view.rs`
- Test: inline for geometry-change handling

**Interfaces:**
- Produces: `App` holds `DockState<ViewId>` and `HashMap<ViewId, ViewRuntime>` where `ViewRuntime { view: View, worker: Option<ChainWorker>, ring: RowRing, waterfall: Waterfall }`. "New view" menu creates a tab bound to a chosen source. Each tab's toolbar: source name (read-only), FFT size ComboBox (256…65536 powers of two), window ComboBox, averaging DragValue 1…256, max-hold toggle + reset button, sub-band controls (offset DragValue in Hz, decimation ComboBox, "Off"), ref level / range DragValues, mode (Spectrum/Waterfall/Both). Changing any chain setting calls `worker.set_config`. When a row arrives whose `span_hz` or bin count differs from the previous one, the ring is cleared and the waterfall texture is reallocated (Review Focus 2).

- [ ] **Step 1: Write failing test** `geometry_change_clears_ring`: `ViewRuntime::ingest(frame)` with span 2e6 then span 1e6 → `ring.len() == 1` after the second frame and `waterfall.rows_pushed() == 1`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** One `ChainWorker` per view, each with its own `source.subscribe(32)`. Closing a tab drops its worker. Closing a source closes its views.
- [ ] **Step 4: Run** → PASS. Manual: HackRF with two views (FFT 4096 avg 8; FFT 1024 sub-band ±2 MHz decimation 4) plus a SigMF replay view in a third tab, all at 60 fps, status shows `drops 0` for 10 minutes (spec success criterion).
- [ ] **Step 5: Commit** `feat(app): tiled multi-view with per-view chain settings`.

---

### Task 11: Markers and delta readout

**Files:**
- Modify: `crates/app/src/view.rs`, `crates/app/src/spectrum_plot.rs`
- Test: inline

**Interfaces:**
- Produces:
  ```rust
  pub struct Marker { pub index: u8, pub hz: f64 }
  impl View {
      pub fn add_marker_at(&mut self, hz: f64) -> u8;     // next free index 1..=9
      pub fn remove_marker(&mut self, index: u8);
      pub fn snap_marker_to_peak(&mut self, index: u8, row: &SpectrumRow, search_bins: usize);
  }
  pub fn marker_readout(markers: &[Marker], row: &SpectrumRow) -> Vec<MarkerReadout>;   // includes delta of M2−M1 when both exist
  pub struct MarkerReadout { pub index: u8, pub hz: f64, pub db: f32, pub delta_from_first: Option<(f64, f32)> }
  ```
  UI: left click on the spectrum adds a marker; dragging a marker line moves it; right click on a marker removes it; `P` key snaps the hovered marker to the peak within ±20 bins. Readout table in the view toolbar's right side; marker color `#ffd36b`.

- [ ] **Step 1: Write failing tests:** `readout_reports_power_and_delta` (row with known bins; M1 at bin 100, M2 at bin 300 → dB values match, delta == (200·bin_width, db2−db1)); `snap_moves_to_local_peak`.
- [ ] **Step 2: Run** `cargo test -p spectra marker` → FAIL.
- [ ] **Step 3: Implement** the marker API, the click/drag/right-click/`P` interactions in `draw_spectrum`, and the readout table.
- [ ] **Step 4: Run** → PASS; manual check of click, drag, remove and peak snap.
- [ ] **Step 5: Commit** `feat(app): markers with delta readout`.

---

### Task 12: FFT-stream source and documented wire format

**Files:**
- Create: `crates/sources/src/fft_stream.rs`, `crates/sources/examples/fft_stream_producer.rs`, `docs/fft-stream-format.md`
- Modify: `crates/app/src/app.rs` (Source menu: "Listen for FFT stream…", port field default 4590)
- Test: `crates/sources/tests/fft_stream.rs`

**Interfaces:**
- Wire format (little-endian, TCP, one producer per connection):
  ```
  header (once): magic "SPEC" (4 bytes) | version u16 = 1 | units u8 (0 = dBFS, 1 = dBm) | reserved u8 = 0
                 | center_hz f64 | span_hz f64 | bin_count u32
  frame (repeated): timestamp_ns u64 | bin_count × f32 power
  ```
- Produces:
  ```rust
  pub struct FftStreamSource { .. }
  impl FftStreamSource { pub fn listen(addr: SocketAddr) -> anyhow::Result<Self>; pub fn local_addr(&self) -> SocketAddr; }
  impl SpectrumSource for FftStreamSource   // info() reflects the current/last connection header; status Stopped("waiting for producer") until a header arrives
  ```
  For a spectrum source the app creates no `ChainWorker`; `ViewRuntime` wraps each `SpectrumRow` into a `RowFrame` (with `source_dropped_blocks` from the subscriber) and computes `average_db`/`max_hold_db` via a `dsp::TraceAccumulator { pub fn new(averaging, max_hold); pub fn push(&mut self, row: &[f32]) -> (Arc<[f32]>, Option<Arc<[f32]>>) }` (add to `crates/dsp/src/chain.rs`, reused by `Chain`).

- [ ] **Step 1: Write failing tests:** `receives_rows_from_producer` (listen on port 0, connect, send header 100e6/1e6/512 dBm + 3 frames → 3 rows with matching geometry and `units == Dbm`); `rejects_bad_magic_and_accepts_next_connection` (send `"NOPE"` → connection closed by server, status contains `"bad magic"`; a second valid connection streams); `short_frame_closes_connection` (send header then 100 bytes of a 2048-byte frame and close → no row emitted, status `Stopped`, next producer works) (Review Focus 4).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** Accept loop thread; per-connection read with `read_exact`; `bin_count` limit 1_048_576 else reject. Example producer sends a synthetic sweep at 50 rows/s. Write `docs/fft-stream-format.md` from the table above plus the producer example invocation.
- [ ] **Step 4: Run** → PASS. Manual: `cargo run -p spectra-sources --example fft_stream_producer -- 127.0.0.1:4590` while the app listens; a view on that source shows the sweep.
- [ ] **Step 5: Commit** `feat(sources): FFT-stream TCP source with documented format`.

---

### Task 13: Status bar, layout persistence, README

**Files:**
- Create: `crates/app/src/layout.rs`, `README.md`
- Modify: `crates/app/src/app.rs`
- Test: inline for round-trip

**Interfaces:**
- Produces: `pub struct SavedLayout { pub dock: DockState<ViewId>, pub views: Vec<View>, pub sources: Vec<SavedSource> }`, `pub enum SavedSource { Sigmf { path: PathBuf }, Device { args: String, center_hz: f64, sample_rate_hz: f64 }, FftStream { port: u16 } }`, `pub fn save(path: &Path, layout: &SavedLayout) -> anyhow::Result<()>`, `pub fn load(path: &Path) -> anyhow::Result<SavedLayout>`. Path: `dirs::config_dir()/spectra/layout.ron`. Saved on exit, loaded on start; sources that fail to reopen are shown as `Stopped(reason)` tabs rather than aborting. Global status bar: per-view `rows/s`, app fps, `drops N` (sum of worker `input_dropped_blocks`), `OVERLOAD` in red when any worker is overloaded (Review Focus 3), noise-floor estimate (median of the latest row).

- [ ] **Step 1: Write failing test** `layout_round_trips_through_ron`.
- [ ] **Step 2: Run** `cargo test -p spectra layout` → FAIL.
- [ ] **Step 3: Implement** `layout.rs`, the status bar, and `README.md` (what it is, `nix develop` or plain `cargo run`, how to open each source, link to `docs/fft-stream-format.md`).
- [ ] **Step 4: Run** → PASS; `cargo clippy --workspace -- -D warnings` clean; `cargo fmt --check` clean. Manual: quit and relaunch restores tabs and settings.
- [ ] **Step 5: Commit** `feat(app): status bar, layout persistence, README`.
