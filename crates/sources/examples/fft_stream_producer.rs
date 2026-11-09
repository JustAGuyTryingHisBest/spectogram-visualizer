//! Sends a synthetic sweeping spectrum to a listening spectra FFT-stream source.
//! Usage: fft_stream_producer [host:port]   (default 127.0.0.1:4590)
use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4590".into());
    let bin_count: u32 = 1024;
    let mut stream = TcpStream::connect(&addr)?;
    let mut header = Vec::new();
    header.extend_from_slice(b"SPEC");
    header.extend_from_slice(&1u16.to_le_bytes());
    header.push(1); // dBm
    header.push(0);
    header.extend_from_slice(&2.4e9f64.to_le_bytes());
    header.extend_from_slice(&20e6f64.to_le_bytes());
    header.extend_from_slice(&bin_count.to_le_bytes());
    stream.write_all(&header)?;
    println!("streaming {bin_count} bins at 50 rows/s to {addr}; Ctrl-C to stop");

    let mut frame_index = 0u64;
    loop {
        let sweep_bin = (frame_index % u64::from(bin_count)) as f32;
        let mut frame = (frame_index * 20_000_000).to_le_bytes().to_vec();
        frame.extend_from_slice(&bin_count.to_le_bytes());
        for bin in 0..bin_count {
            let distance = (bin as f32 - sweep_bin).abs();
            let power = -100.0
                + 50.0 * (-(distance * distance) / 50.0).exp()
                + (frame_index as f32 * 0.37 + bin as f32 * 1.3).sin() * 1.5;
            frame.extend_from_slice(&power.to_le_bytes());
        }
        stream.write_all(&frame)?;
        frame_index += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
}
