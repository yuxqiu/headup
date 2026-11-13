//! Play a sound on the default audio output and wait until it has been heard.
//!
//! The output is opened for each sound and closed after it. An output that stays open streams
//! silence, which keeps a Bluetooth audio link busy (DECISIONS "Silent lead-in for every sound",
//! rejected alternative). Closing it discards whatever is still buffered, so a sound counts as
//! played only once its last sample has reached the speaker. The output tells when each buffer it
//! asks for will be heard (`cpal`'s playback timestamp). The first buffer asked for after the whole
//! sound was handed over starts right after it, so its playback time is when the sound has been
//! heard.

use super::wav::Pcm;
use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How much audio the output asks for at a time. The output asks for the next buffer while the
/// last one is still queued, so the wait for the end of a sound overshoots by at most this much.
/// Left to itself, a PulseAudio server would use 2 s.
const BUFFER: Duration = Duration::from_millis(20);

/// How much longer than the sound itself playback may take before the output counts as stalled.
/// A stalled or vanished output would otherwise never report the end, and a spoken prompt would
/// hold up the session.
const STALL: Duration = Duration::from_secs(2);

/// Play `pcm` on the default output and return once its last sample has been heard. Blocks the
/// thread. Fails if the output cannot be opened, reports an error, or stalls.
pub fn play(pcm: &Pcm) -> Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .context("no audio output device")?;
    let supported = device
        .default_output_config()
        .context("read the audio output's configuration")?;
    let mut config = supported.config();
    let frames = (f64::from(config.sample_rate) * BUFFER.as_secs_f64()) as cpal::FrameCount;
    config.buffer_size = match *supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            cpal::BufferSize::Fixed(frames.clamp(min, max))
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Fixed(frames),
    };
    let samples = convert(pcm, config.sample_rate, config.channels);
    match supported.sample_format() {
        SampleFormat::F32 => play_as::<f32>(&device, config, samples),
        SampleFormat::I16 => play_as::<i16>(&device, config, samples),
        SampleFormat::I32 => play_as::<i32>(&device, config, samples),
        SampleFormat::U16 => play_as::<u16>(&device, config, samples),
        other => bail!("audio output uses an unsupported sample format ({other})"),
    }
}

/// Play interleaved `samples` on `device` in its sample type `T`; see [`play`].
fn play_as<T>(device: &cpal::Device, config: StreamConfig, samples: Vec<f32>) -> Result<()>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let rate = f64::from(config.sample_rate);
    let length = Duration::from_secs_f64(samples.len() as f64 / channels as f64 / rate);
    // When the last sample will be heard, or why playback failed.
    let (end_tx, end_rx) = mpsc::channel::<Result<Instant>>();
    let error_tx = end_tx.clone();
    let mut next = 0;
    let mut reported = false;
    // The sound's samples, then silence.
    let stream = device
        .build_output_stream::<T, _, _>(
            config,
            move |out: &mut [T], info| {
                if next == samples.len() && !reported {
                    // The whole sound was handed over before this buffer, which is heard from
                    // its playback time on.
                    reported = true;
                    let t = info.timestamp();
                    let heard = t.playback.duration_since(t.callback);
                    let _ = end_tx.send(Ok(Instant::now() + heard));
                }
                let n = (samples.len() - next).min(out.len());
                for (o, &s) in out.iter_mut().zip(&samples[next..next + n]) {
                    *o = T::from_sample(s);
                }
                for o in &mut out[n..] {
                    *o = T::EQUILIBRIUM;
                }
                next += n;
            },
            move |e| {
                let _ = error_tx.send(Err(anyhow!(e).context("audio output")));
            },
            Some(STALL),
        )
        .context("open the audio output")?;
    stream.play().context("start the audio output")?;
    let end = end_rx
        .recv_timeout(length + STALL)
        .map_err(|_| anyhow!("audio output stalled: the sound did not finish in time"))??;
    std::thread::sleep(end.saturating_duration_since(Instant::now()));
    Ok(())
}

/// `pcm` as interleaved samples for an output with `rate` and `channels`: resampled by linear
/// interpolation, with the same signal on every channel.
fn convert(pcm: &Pcm, rate: u32, channels: u16) -> Vec<f32> {
    let src: Vec<f32> = pcm
        .samples
        .iter()
        .map(|&s| f32::from(s) / 32768.0)
        .collect();
    let frames = (src.len() as u64 * u64::from(rate) / u64::from(pcm.rate)) as usize;
    let step = f64::from(pcm.rate) / f64::from(rate);
    (0..frames)
        .flat_map(|i| {
            let x = i as f64 * step;
            let j = x as usize;
            let a = src[j];
            let b = src.get(j + 1).copied().unwrap_or(a);
            let v = a + (b - a) * (x - j as f64) as f32;
            std::iter::repeat_n(v, usize::from(channels))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(rate: u32, samples: &[i16]) -> Pcm {
        Pcm {
            rate,
            samples: samples.to_vec(),
        }
    }

    #[test]
    fn same_rate_and_mono_keeps_the_samples() {
        let out = convert(&pcm(48_000, &[0, 16_384, -16_384]), 48_000, 1);
        assert_eq!(out, [0.0, 0.5, -0.5]);
    }

    #[test]
    fn double_rate_interpolates_between_samples() {
        let out = convert(&pcm(24_000, &[0, 16_384]), 48_000, 1);
        assert_eq!(out, [0.0, 0.25, 0.5, 0.5]);
    }

    #[test]
    fn every_channel_gets_the_signal() {
        let out = convert(&pcm(48_000, &[16_384, 0]), 48_000, 2);
        assert_eq!(out, [0.5, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn length_follows_the_rate() {
        let one_second = pcm(22_050, &[0; 22_050]);
        assert_eq!(convert(&one_second, 48_000, 2).len(), 2 * 48_000);
    }
}
