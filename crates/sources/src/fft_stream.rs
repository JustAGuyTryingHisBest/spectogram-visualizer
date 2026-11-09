use std::io::{self, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use spectra_core::{
    Broadcast, PowerUnits, SourceStatus, SpectrumRow, SpectrumSource, StreamInfo, Subscriber,
};

const MAGIC: &[u8; 4] = b"SPEC";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 4 + 2 + 1 + 1 + 8 + 8 + 4;
const MAX_BINS: u32 = 1_048_576;

struct Shared {
    info: Mutex<StreamInfo>,
    status: Mutex<SourceStatus>,
    stop: AtomicBool,
}

/// Accepts one producer at a time over TCP and republishes its spectrum rows.
/// Wire format: see `docs/fft-stream-format.md`.
pub struct FftStreamSource {
    local_addr: SocketAddr,
    shared: Arc<Shared>,
    broadcast: Arc<Broadcast<SpectrumRow>>,
}

impl FftStreamSource {
    pub fn listen(addr: SocketAddr) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).with_context(|| format!("binding {addr}"))?;
        let local_addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            info: Mutex::new(StreamInfo {
                name: format!("fft-stream :{}", local_addr.port()),
                center_hz: 0.0,
                sample_rate_hz: 0.0,
                calibration_offset_db: None,
            }),
            status: Mutex::new(SourceStatus::Stopped("waiting for producer".into())),
            stop: AtomicBool::new(false),
        });
        let broadcast = Arc::new(Broadcast::new());
        let accept_shared = Arc::clone(&shared);
        let accept_broadcast = Arc::clone(&broadcast);
        listener.set_nonblocking(true)?;
        thread::Builder::new()
            .name("fft-stream-accept".into())
            .spawn(move || accept_loop(listener, accept_shared, accept_broadcast))?;
        Ok(Self {
            local_addr,
            shared,
            broadcast,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for FftStreamSource {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl SpectrumSource for FftStreamSource {
    fn info(&self) -> StreamInfo {
        self.shared.info.lock().unwrap().clone()
    }

    fn subscribe(&self) -> Subscriber<SpectrumRow> {
        self.broadcast.subscribe(64)
    }

    fn status(&self) -> SourceStatus {
        self.shared.status.lock().unwrap().clone()
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, broadcast: Arc<Broadcast<SpectrumRow>>) {
    while !shared.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let outcome = serve_connection(stream, &shared, &broadcast);
                let reason = match outcome {
                    Ok(()) => "producer disconnected".to_string(),
                    Err(error) => format!("{error:#}"),
                };
                *shared.status.lock().unwrap() = SourceStatus::Stopped(reason);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                *shared.status.lock().unwrap() = SourceStatus::Stopped(error.to_string());
                return;
            }
        }
    }
}

struct Header {
    units: PowerUnits,
    center_hz: f64,
    span_hz: f64,
    bin_count: usize,
}

fn parse_header(bytes: &[u8; HEADER_LEN]) -> anyhow::Result<Header> {
    if &bytes[0..4] != MAGIC {
        bail!("bad magic {:?}", &bytes[0..4]);
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != VERSION {
        bail!("unsupported version {version}");
    }
    let units = match bytes[6] {
        0 => PowerUnits::Dbfs,
        1 => PowerUnits::Dbm,
        other => bail!("unknown units code {other}"),
    };
    let center_hz = f64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let span_hz = f64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let bin_count = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    if bin_count == 0 || bin_count > MAX_BINS {
        bail!("bin count {bin_count} out of range 1..={MAX_BINS}");
    }
    if !(span_hz > 0.0 && span_hz.is_finite() && center_hz.is_finite()) {
        bail!("invalid geometry center {center_hz} span {span_hz}");
    }
    Ok(Header {
        units,
        center_hz,
        span_hz,
        bin_count: bin_count as usize,
    })
}

/// Reads with a timeout, retrying on timeouts until data arrives or the source stops.
/// `Ok(false)` means the peer closed before sending any byte of this read (a clean boundary).
fn read_exact_patient(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    shared: &Shared,
) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        if shared.stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "source stopped"));
        }
        match stream.read(&mut buffer[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("short read: {filled} of {} bytes", buffer.len()),
                ));
            }
            Ok(count) => filled += count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

fn serve_connection(
    mut stream: TcpStream,
    shared: &Shared,
    broadcast: &Broadcast<SpectrumRow>,
) -> anyhow::Result<()> {
    let mut header_bytes = [0u8; HEADER_LEN];
    if !read_exact_patient(&mut stream, &mut header_bytes, shared).context("reading header")? {
        bail!("producer closed before sending a header");
    }
    let header = parse_header(&header_bytes)?;
    {
        let mut info = shared.info.lock().unwrap();
        info.center_hz = header.center_hz;
        info.sample_rate_hz = header.span_hz;
        info.calibration_offset_db = (header.units == PowerUnits::Dbm).then_some(0.0);
    }
    *shared.status.lock().unwrap() = SourceStatus::Streaming;

    let mut frame_bytes = vec![0u8; 8 + header.bin_count * 4];
    loop {
        if !read_exact_patient(&mut stream, &mut frame_bytes, shared).context("reading frame")? {
            return Ok(());
        }
        let power_db: Vec<f32> = frame_bytes[8..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        broadcast.publish(SpectrumRow {
            center_hz: header.center_hz,
            span_hz: header.span_hz,
            units: header.units,
            power_db: power_db.into(),
            received_at: Instant::now(),
        });
    }
}
