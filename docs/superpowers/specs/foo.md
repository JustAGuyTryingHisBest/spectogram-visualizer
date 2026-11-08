# Spectrum analyzer / waterfall tool — design

Date: 2026-11-07
Status: draft for review

## Intent

A free, fast spectrum analyzer and waterfall for macOS, in the spirit of
Aaronia RTSA Suite and 3dB Labs Spectre, for hunting unknown signals across
a wide span. The two commercial tools have good GUIs and great performance;
their problems are license cost and second-class macOS support. This tool
exists to remove both without giving up the speed.

## Goals

- Live spectrum plot plus scrolling waterfall from any attached radio or a
  recorded IQ file, at a steady 60 fps on an Apple Silicon Mac.
- Radio agnostic. The core never knows which device it is reading. Each
  device is one implementation of an in-codebase source interface
  ("plugin" here means a trait implementation compiled into the binary,
  not a runtime-loadable module).
- Multiple views at once: several spectrum/waterfall blocks on screen,
  fed by the same radio with different processing settings, or by
  different radios side by side.
- Performance and GUI quality on par with RTSA Suite / Spectre. Speed
  matters more than native macOS look and feel.

## Non-goals (first version)

- Demodulation, audio output, or protocol decoding.
- Sweeping and stitching spans wider than a device's instantaneous
  bandwidth.
- VITA 49 input.
- Native Aaronia Spectran V6 driver. Aaronia's SDK is not a target; V6 data
  arrives, if at all, through the generic FFT-stream source.
- Runtime plugin loading, scripting, or a third-party plugin API.
- Automatic signal detection, waterfall history scrubbing, IQ recording,
  drag-to-zoom/retune, and persistence (phosphor) display. These were
  discussed and deferred to later versions; the architecture should not
  preclude them (see "Design for later" below).

## Anti-goals

- A UI that stutters, drops frames, or lags behind the device. If a view
  cannot keep up it should degrade visibly (drop rows, show a counter),
  never stall the device reader or other views.
- Device-specific concepts leaking into the display or DSP layers.
- macOS as a port. macOS is the primary development and test platform.

## Constraints

- Language: Rust, single binary, Cargo workspace.
- UI: egui rendered through wgpu (Metal backend on macOS).
- Dev environment: Nix flake providing Rust toolchain and the C device
  libraries. Device libraries may also come from Homebrew; the build must
  not require Nix.
- Other platforms (Linux, Windows) are welcome if they fall out of the
  choices above, but nothing is traded off for them.

## Architecture

Three layers connected by streams. Data flows strictly downward.

```
 Sources           Processing chains            Views
 (one per device   (one per view)               (one per tile)
  or file)

 HackRF ───┬────▶ chain A (FFT 4096, avg 8) ───▶ view 1: spectrum + waterfall
           └────▶ chain B (FFT 1024, sub-band) ─▶ view 2: spectrum
 RTL-SDR ───────▶ chain C ───────────────────────▶ view 3: waterfall
 SigMF file ────▶ chain D ───────────────────────▶ view 4
 FFT stream ────▶ chain E (avg only) ────────────▶ view 5
```

### Sources

A source is one open device or file with one tuning state. Two source
traits share common metadata (center frequency, sample rate, timestamp,
gain, device name):

- **IQ source** yields blocks of complex samples. Implementations in the
  first version: HackRF (`libhackrf`), Ettus USRP B205-mini (UHD),
  RTL-SDR (`librtlsdr`), SigMF file replay. Build order follows the
  hardware on the owner's desk: HackRF first, then B205-mini, then RTL-SDR.
- **Spectrum source** yields finished power-vs-frequency rows. First
  implementation: a generic FFT-stream input (see "FFT-stream input
  format" below).

A source fans its output out to any number of subscribers so one device
feeds many chains with a single read. Subscribers that fall behind lose
blocks; the source never blocks on a slow consumer. Each source exposes
its tunable parameters (frequency, sample rate, gain stages, bias-T,
antenna port) as a typed, device-described parameter list so the UI can
render a settings panel without knowing the device.

SigMF replay behaves like a live radio plus pause, seek, loop, and
playback speed. It is also the test harness: the core must be testable
with no hardware attached.

### Processing chains

One chain per view. A chain subscribes to one source and emits spectrum
rows. For an IQ source the stages are: optional sub-band selection
(digital down-conversion and decimation to view a slice of the capture),
window function, FFT, power in dBm or dBFS, averaging, max-hold. For a
spectrum source only averaging and max-hold apply. Chains run on worker
threads, independent of the device reader and the UI thread.

Chains are gapless: every IQ sample the source delivers is windowed and
transformed, so max-hold and average traces see every burst. The chain
may produce far more rows than the display can show; the view combines or
drops rows only at the display stage. If a chain cannot keep up with its
source on the available CPU, that is an overload condition surfaced in
the status bar, not silently skipped. "Zero dropped blocks" in the success
criteria means zero drops between device reader and chain.

Each chain exposes: FFT size, window (Blackman-Harris default, plus Hann,
Hamming, rectangular, flat-top), averaging count, max-hold on/off and
reset, and the sub-band selection.

### Views

A view is a spectrum plot, a waterfall, or both stacked, drawn from one
chain's rows. The window holds a tiled layout of views the user can split,
resize, and close. Each view's own toolbar names its source and chain
settings. Source tuning controls live in a per-source panel, because
changing them affects every view on that source.

Spectrum plot: live trace, average trace, max-hold trace, each toggleable
with distinct colors; dB and frequency axes with grid; reference level and
range. Markers: click to place, numbered, readout of frequency and power,
delta readout between two markers, drag to move, peak-snap on request.

Waterfall: rows scroll from top (newest) to bottom, with a time axis,
colormap bar, selectable colormap (a perceptual default such as inferno
or viridis), and the same reference/range as the spectrum. The waterfall
is a GPU texture updated one row per new spectrum row; the CPU does not
redraw history.

Status bar per view or global: streaming state, FFT rows per second,
frames per second, dropped blocks, estimated noise floor.

### Threading and data flow

- Device reader thread per source, as close to the C library callback as
  possible, doing nothing but handing blocks to subscribers.
- Worker thread(s) for chains.
- UI thread owns egui/wgpu and only consumes finished rows.
- Hand-offs use bounded channels or a ring with drop-oldest semantics.
  Nothing upstream waits on anything downstream.

### Design for later

The first version does not build these, but the structure should leave room:

- Waterfall history scrub requires keeping a ring of rows per view rather
  than only the texture. Keep rows in a ring buffer from the start even if
  nothing reads it yet.
- Detections and recording both hang off the chain and source stages
  respectively, as additional subscribers.
- Popping a view into a separate OS window is possible with egui viewports.

## Decisions already made (as the owner made them)

| Topic | Decision |
|---|---|
| License cost | Free, no third-party license |
| Primary platform | macOS |
| Plugin model | In-codebase trait implementations |
| Sources, v1 | HackRF, USRP B205-mini, RTL-SDR, SigMF replay, generic FFT-stream input |
| Chain processing | Gapless: every IQ sample is transformed; drops only at display |
| Spectran V6 | Not the owner's day-to-day radio; FFT-stream input is a thin, late item tested with a synthetic producer |
| Dev hardware | HackRF One and Ettus B205-mini on the desk |
| VITA 49 | Out |
| Spectran V6 SDK | Out; V6 only via FFT-stream input |
| Primary use | Hunting unknown signals across a wide span |
| Day-one display features | Max-hold + average traces; markers with deltas; multiple views |
| Deferred display features | Detections list, history scrub, IQ recording, drag-to-zoom, persistence |
| Multiple views | Same radio with different chain settings, and different radios side by side |
| Language / UI | Rust, egui on wgpu (owner accepted my proposal) |
| Priority | Speed over native appearance |

## Calls I made (owner can veto)

- USRP B205-mini is in v1 because the owner has one for development; the
  owner only named it as dev hardware, so cut it if UHD on macOS proves
  painful.
- Views on the same radio share tuning (center, rate, gain) and differ
  only downstream, including digital sub-band selection.
- Single window with a saved tiled layout; layout and last settings
  persist across runs in a config file.
- Power units default to dBFS, with dBm available when a source reports a
  calibration offset.
- Default window Blackman-Harris, default FFT 2048, default averaging 8.
- No sweeping; a view's span equals the source sample rate or a sub-band
  of it.
- FFT-stream input format: a simple length-prefixed binary frame over TCP
  or a UNIX socket, each frame carrying center frequency, span, timestamp,
  bin count, and float32 power per bin. Documented in the repo so other
  tools (including Aaronia's software via a small shim) can produce it.
  Chosen over a standard because no widely used standard exists outside
  VITA 49, which is out.
- FFT-stream geometry (center, span, bin count) is fixed for the life of a
  connection and sent once in a connection header. A producer that needs
  to change geometry reconnects. Per-frame fields are timestamp and power
  only. This keeps the waterfall texture and axes static per source.
- The FFT-stream connection header declares its power units, dBm or dBFS,
  and the chain never rescales spectrum-source rows. A producer that does
  not know its calibration sends dBFS.

## Success criteria

(My reading of the conversation; the owner confirmed gapless processing
but did not state the rest directly.)

- On an Apple Silicon Mac with a HackRF at 20 MS/s, two views of the same
  device plus one SigMF replay view all render at 60 fps with zero blocks
  dropped between device reader and chains for ten minutes.
- Opening a SigMF recording and hunting through it works with no hardware
  attached.
- Adding a new radio means implementing one trait in one new module and
  registering it; nothing in DSP or UI changes.
- The owner reaches for this instead of RTSA Suite or Spectre for
  wide-span hunting on the Mac.

## Left to the builder

- Crate layout within the workspace (suggested: `source`, `dsp`, `app`,
  one crate or module per device).
- FFI strategy for `librtlsdr` and `libhackrf` (existing crates vs
  `bindgen`), and how to locate the libraries on macOS.
- Choice of FFT crate and whether to use Apple Accelerate.
- Exact channel/ring buffer types and sizes.
- Colormap set beyond one perceptual default.
- Keyboard shortcuts, theming, icon.
- Test strategy; SigMF replay with synthetic files is the intended basis.
- Nix flake contents and whether to also provide a Homebrew-based setup.
