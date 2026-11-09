//! Writes a synthetic SigMF recording: two tones plus noise, 2 MS/s, cf32_le.
//! Usage: make_sigmf <output-stem> [seconds]
use std::f32::consts::TAU;
use std::fs;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let stem = args.next().unwrap_or_else(|| "/tmp/test".into());
    let seconds: f64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(10.0);
    let sample_rate = 2e6;
    let sample_count = (sample_rate * seconds) as usize;

    let mut data = fs::File::create(format!("{stem}.sigmf-data"))?;
    let mut noise_state: u32 = 0x1234_5678;
    let mut noise = move || {
        noise_state ^= noise_state << 13;
        noise_state ^= noise_state >> 17;
        noise_state ^= noise_state << 5;
        (noise_state as f32 / u32::MAX as f32 - 0.5) * 0.02
    };
    let mut buffer = Vec::with_capacity(8 * 65536);
    for index in 0..sample_count {
        let time = index as f32 / sample_rate as f32;
        let tone_a = 0.3 * (TAU * 250e3 * time).sin();
        let tone_b = 0.1 * (TAU * -600e3 * time).sin();
        let bursting = ((time * 2.0).floor() as u32).is_multiple_of(2);
        let tone_c = if bursting {
            0.2 * (TAU * 700e3 * time).sin()
        } else {
            0.0
        };
        let re = tone_a + tone_b + tone_c + noise();
        let im = 0.3 * (TAU * 250e3 * time).cos()
            + 0.1 * (TAU * -600e3 * time).cos()
            + if bursting {
                0.2 * (TAU * 700e3 * time).cos()
            } else {
                0.0
            }
            + noise();
        buffer.extend_from_slice(&re.to_le_bytes());
        buffer.extend_from_slice(&im.to_le_bytes());
        if buffer.len() >= 8 * 65536 {
            data.write_all(&buffer)?;
            buffer.clear();
        }
    }
    data.write_all(&buffer)?;

    fs::write(
        format!("{stem}.sigmf-meta"),
        format!(
            r#"{{"global":{{"core:datatype":"cf32_le","core:sample_rate":{sample_rate},"core:version":"1.0.0","core:description":"synthetic two tones + burst"}},"captures":[{{"core:sample_start":0,"core:frequency":915000000.0}}],"annotations":[]}}"#
        ),
    )?;
    println!("wrote {stem}.sigmf-meta ({sample_count} samples)");
    Ok(())
}
