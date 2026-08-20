use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use parking_lot::Mutex;

pub const ENGINE_SAMPLE_RATE: u32 = 16_000;
// Default Windows input callbacks are ~10 ms after resampling; 256 chunks retain ~2.5 s.
// A full queue is surfaced as a session failure instead of silently dropping speech.
const PREBUFFER_CHUNK_CAPACITY: usize = 256;

#[derive(Debug)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub input_level: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioCaptureFailure {
    PrebufferOverflow,
    Stream { error: String },
}

impl fmt::Display for AudioCaptureFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrebufferOverflow => formatter.write_str("prebuffer overflowed"),
            Self::Stream { error } => write!(formatter, "stream failed: {error}"),
        }
    }
}

pub struct AudioCapture {
    // Keeping this stream alive owns the CPAL callback and stops it on drop.
    #[allow(dead_code)]
    stream: Stream,
    prebuffer_overflowed: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
}

impl AudioCapture {
    pub fn start() -> Result<(Self, Receiver<AudioChunk>)> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No microphone input device is available")?;
        let supported = device
            .default_input_config()
            .context("Read the default microphone format")?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let channels = usize::from(config.channels);
        let source_rate = config.sample_rate.0;
        let (sender, receiver) = mpsc::sync_channel(PREBUFFER_CHUNK_CAPACITY);
        let prebuffer_overflowed = Arc::new(AtomicBool::new(false));
        let stream_error = Arc::new(Mutex::new(None));

        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32, _>(
                &device,
                &config,
                sender,
                Arc::clone(&prebuffer_overflowed),
                Arc::clone(&stream_error),
                move |value| value,
                channels,
                source_rate,
            ),
            SampleFormat::I16 => build_stream::<i16, _>(
                &device,
                &config,
                sender,
                Arc::clone(&prebuffer_overflowed),
                Arc::clone(&stream_error),
                move |value| value as f32 / i16::MAX as f32,
                channels,
                source_rate,
            ),
            SampleFormat::U16 => build_stream::<u16, _>(
                &device,
                &config,
                sender,
                Arc::clone(&prebuffer_overflowed),
                Arc::clone(&stream_error),
                move |value| (value as f32 / u16::MAX as f32) * 2.0 - 1.0,
                channels,
                source_rate,
            ),
            format => bail!("Unsupported microphone sample format: {format:?}"),
        }?;
        stream.play().context("Start microphone capture")?;
        Ok((
            Self {
                stream,
                prebuffer_overflowed,
                stream_error,
            },
            receiver,
        ))
    }

    pub fn failure(&self) -> Option<AudioCaptureFailure> {
        if self.prebuffer_overflowed.load(Ordering::Acquire) {
            return Some(AudioCaptureFailure::PrebufferOverflow);
        }
        self.stream_error
            .lock()
            .clone()
            .map(|error| AudioCaptureFailure::Stream { error })
    }
}

fn build_stream<T, F>(
    device: &cpal::Device,
    config: &StreamConfig,
    sender: SyncSender<AudioChunk>,
    prebuffer_overflowed: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
    convert: F,
    channels: usize,
    source_rate: u32,
) -> Result<Stream>
where
    T: cpal::SizedSample + Send + 'static,
    F: Fn(T) -> f32 + Send + Sync + 'static,
{
    let resampler = Mutex::new(LinearResampler::new(source_rate, ENGINE_SAMPLE_RATE));
    let stream = device
        .build_input_stream(
            config,
            move |interleaved: &[T], _| {
                let mono = mix_to_mono(interleaved, channels, &convert);
                if mono.is_empty() {
                    return;
                }
                let input_level = rms_level(&mono);
                let samples = resampler.lock().push(&mono);
                if !samples.is_empty() {
                    match sender.try_send(AudioChunk {
                        samples,
                        input_level,
                    }) {
                        Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                        Err(TrySendError::Full(_)) => {
                            prebuffer_overflowed.store(true, Ordering::Release);
                        }
                    }
                }
            },
            move |error| {
                log::error!("Microphone stream failed: {error}");
                *stream_error.lock() = Some(error.to_string());
            },
            None,
        )
        .context("Open microphone stream")?;
    Ok(stream)
}

fn mix_to_mono<T, F>(interleaved: &[T], channels: usize, convert: &F) -> Vec<f32>
where
    T: Copy,
    F: Fn(T) -> f32,
{
    if channels == 0 {
        return Vec::new();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().map(convert).sum::<f32>() / channels as f32)
        .collect()
}

fn rms_level(samples: &[f32]) -> f32 {
    let energy = samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
    (energy.sqrt() * 4.0).clamp(0.0, 1.0)
}

#[derive(Debug)]
struct LinearResampler {
    ratio: f64,
    position: f64,
    source: Vec<f32>,
}

impl LinearResampler {
    fn new(source_rate: u32, output_rate: u32) -> Self {
        Self {
            ratio: source_rate as f64 / output_rate as f64,
            position: 0.0,
            source: Vec::new(),
        }
    }

    fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        self.source.extend_from_slice(samples);
        let mut output = Vec::with_capacity((samples.len() as f64 / self.ratio).ceil() as usize);
        while self.position + 1.0 < self.source.len() as f64 {
            let left = self.position.floor() as usize;
            let fraction = (self.position - left as f64) as f32;
            output.push(self.source[left] + (self.source[left + 1] - self.source[left]) * fraction);
            self.position += self.ratio;
        }

        let removable = (self.position.floor() as usize).saturating_sub(1);
        if removable > 0 {
            self.source.drain(..removable);
            self.position -= removable as f64;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_frames_are_mixed_without_changing_frame_count() {
        let mono = mix_to_mono(&[1.0_f32, -1.0, 0.5, 0.5], 2, &|value| value);
        assert_eq!(mono, vec![0.0, 0.5]);
    }

    #[test]
    fn forty_eight_khz_is_reduced_to_sixteen_khz() {
        let mut resampler = LinearResampler::new(48_000, 16_000);
        let source = vec![0.25; 48_000];
        let output = resampler.push(&source);
        assert!(
            (15_998..=16_001).contains(&output.len()),
            "{}",
            output.len()
        );
        assert!(
            output
                .iter()
                .all(|sample| (*sample - 0.25).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn level_is_bounded() {
        assert_eq!(rms_level(&[1.0, -1.0]), 1.0);
        assert!(rms_level(&[0.05, -0.05]) > 0.0);
    }
}
