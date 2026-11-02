//! System-audio (WASAPI loopback on Windows; selected CoreAudio input on macOS)
//! and optional microphone capture with a simple
//! energy-based voice activity detector. Finished utterances (16 kHz mono i16)
//! are sent to the transcription pipeline. Audio never touches the disk.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::atomic::AtomicUsize;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast::Sender;

use crate::Shared;

pub const RATE: u32 = 16_000;
pub const UTTERANCE_QUEUE_CAPACITY: usize = 4;
const AUDIO_BUFFER_CAPACITY: usize = 16;
const AUDIO_CHUNK_SAMPLES: usize = 1024;
const FRAME: usize = 320; // 20 ms at 16 kHz
const FRAME_MS: u32 = 20;
const PREROLL_FRAMES: usize = 15; // 300 ms kept before speech onset
const ONSET_FRAMES: u32 = 3; // 60 ms of voice to start an utterance
const TAIL_KEEP_MS: u32 = 300;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Source {
    System,
    Mic,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::System => "system",
            Source::Mic => "mic",
        }
    }
}

pub struct Utterance {
    pub pcm: Vec<i16>,
    pub queued_at: Instant,
}

// Inline storage keeps the capture callback free of heap allocations.
struct AudioChunk {
    samples: [f32; AUDIO_CHUNK_SAMPLES],
    len: usize,
}

fn mono_chunks<T>(data: &[T], channels: usize) -> impl Iterator<Item = AudioChunk> + '_
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = channels.max(1);
    data.chunks(AUDIO_CHUNK_SAMPLES * channels).map(move |input| {
        let mut mono = AudioChunk { samples: [0.0; AUDIO_CHUNK_SAMPLES], len: 0 };
        for c in input.chunks(channels) {
            mono.samples[mono.len] =
                c.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / c.len() as f32;
            mono.len += 1;
        }
        mono
    })
}

pub fn spawn(app: AppHandle, shared: Arc<Shared>, tx: Sender<Arc<Utterance>>) {
    #[cfg(target_os = "macos")]
    let sources = [Source::System];
    #[cfg(not(target_os = "macos"))]
    let sources = [Source::System, Source::Mic];
    for src in sources {
        let (app, shared, tx) = (app.clone(), shared.clone(), tx.clone());
        std::thread::Builder::new()
            .name(format!("audio-{}", src.name()))
            .spawn(move || capture_loop(src, app, shared, tx))
            .expect("spawn audio thread");
    }
}

fn wanted(src: Source, shared: &Shared) -> bool {
    match src {
        Source::System => true,
        Source::Mic => shared.settings.lock().unwrap().use_mic,
    }
}

fn default_device(src: Source) -> Option<(cpal::Device, String)> {
    let host = cpal::default_host();
    let dev = match src {
        Source::System => {
            #[cfg(target_os = "macos")]
            {
                host.default_input_device()
            }
            #[cfg(not(target_os = "macos"))]
            {
                host.default_output_device()
            }
        }
        Source::Mic => host.default_input_device(),
    }?;
    let name = dev.name().unwrap_or_default();
    Some((dev, name))
}

/// Keeps a capture stream open on the current default device, reopening it when the
/// device changes or the stream reports an error.
fn capture_loop(src: Source, app: AppHandle, shared: Arc<Shared>, tx: Sender<Arc<Utterance>>) {
    let mut reported = false;
    loop {
        if !wanted(src, &shared) {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let fail = |msg: String, reported: &mut bool| {
            if !*reported && src == Source::System {
                crate::set_status(&app, &shared, "error", &msg);
            }
            *reported = true;
            std::thread::sleep(Duration::from_secs(2));
        };
        let Some((device, name)) = default_device(src) else {
            fail(
                match src {
                    Source::System => {
                        #[cfg(target_os = "macos")]
                        {
                            "Нет устройства ввода системного звука (настройте BlackHole или другой loopback-вход)".into()
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            "Нет устройства вывода звука".into()
                        }
                    }
                    Source::Mic => "Нет микрофона".into(),
                },
                &mut reported,
            );
            continue;
        };
        let (data_tx, data_rx) = mpsc::sync_channel::<AudioChunk>(AUDIO_BUFFER_CAPACITY);
        let dropped_chunks = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let (stream, rate) = match build_stream(
            src,
            &device,
            data_tx,
            dropped_chunks.clone(),
            failed.clone(),
        ) {
            Ok(v) => v,
            Err(e) => {
                fail(format!("Ошибка захвата звука: {e}"), &mut reported);
                continue;
            }
        };
        if let Err(e) = stream.play() {
            fail(format!("Ошибка захвата звука: {e}"), &mut reported);
            continue;
        }
        if reported || src == Source::System {
            crate::base_status(&app, &shared);
        }
        reported = false;

        let mut proc = Processor::new(rate);
        let mut last_check = Instant::now();
        let ctx = Ctx { src, app: &app, shared: &shared, tx: &tx };
        loop {
            match data_rx.recv_timeout(Duration::from_millis(150)) {
                Ok(buf) => proc.push(&buf.samples[..buf.len], &ctx),
                // Loopback devices may deliver nothing while nothing plays; treat it
                // as silence so a pending utterance still gets finalized.
                Err(mpsc::RecvTimeoutError::Timeout) => proc.push_silence(150, &ctx),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            let dropped = dropped_chunks.swap(0, Ordering::Relaxed);
            if dropped > 0 {
                eprintln!("[audio] dropped {dropped} audio buffers because processing fell behind");
            }
            if failed.load(Ordering::Relaxed) {
                break;
            }
            if last_check.elapsed() >= Duration::from_secs(1) {
                last_check = Instant::now();
                if !wanted(src, &shared) {
                    break;
                }
                match default_device(src) {
                    Some((_, n)) if n == name => {}
                    _ => break, // default device changed or vanished
                }
            }
        }
        drop(stream);
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn build_stream(
    src: Source,
    device: &cpal::Device,
    tx: mpsc::SyncSender<AudioChunk>,
    dropped_chunks: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
) -> Result<(Stream, u32), String> {
    let cfg = match src {
        Source::System => {
            #[cfg(target_os = "macos")]
            {
                device.default_input_config()
            }
            #[cfg(not(target_os = "macos"))]
            {
                device.default_output_config()
            }
        }
        Source::Mic => device.default_input_config(),
    }
    .map_err(|e| e.to_string())?;
    let rate = cfg.sample_rate().0;
    let channels = cfg.channels() as usize;
    let format = cfg.sample_format();
    let config: StreamConfig = cfg.into();
    let callback_failed = failed.clone();
    let err_fn = move |_e: cpal::StreamError| failed.store(true, Ordering::Relaxed);
    let stream = match format {
        SampleFormat::F32 => build::<f32>(device, &config, channels, tx, dropped_chunks, callback_failed, err_fn),
        SampleFormat::I16 => build::<i16>(device, &config, channels, tx, dropped_chunks, callback_failed, err_fn),
        SampleFormat::U16 => build::<u16>(device, &config, channels, tx, dropped_chunks, callback_failed, err_fn),
        SampleFormat::I32 => build::<i32>(device, &config, channels, tx, dropped_chunks, callback_failed, err_fn),
        other => return Err(format!("формат {other:?} не поддерживается")),
    }
    .map_err(|e| e.to_string())?;
    Ok((stream, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    tx: mpsc::SyncSender<AudioChunk>,
    dropped_chunks: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = channels.max(1);
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            for mono in mono_chunks(data, channels) {
                match tx.try_send(mono) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(_)) => {
                        dropped_chunks.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        failed.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        },
        err_fn,
        None,
    )
}

struct Ctx<'a> {
    src: Source,
    app: &'a AppHandle,
    shared: &'a Shared,
    tx: &'a Sender<Arc<Utterance>>,
}

struct Processor {
    // integrate-and-dump resampler to 16 kHz
    ratio: f64,
    phase: f64,
    acc: f32,
    cnt: u32,
    frame: [f32; FRAME],
    frame_len: usize,
    // VAD
    smooth: f32,
    onset: u32,
    in_speech: bool,
    preroll: VecDeque<[f32; FRAME]>,
    buf: Vec<f32>,
    voiced_ms: u32,
    silence_ms: u32,
    // level meter for the settings window
    level_max: f32,
    level_frames: u32,
}

impl Processor {
    fn new(rate: u32) -> Self {
        Self {
            ratio: rate as f64 / RATE as f64,
            phase: 0.0,
            acc: 0.0,
            cnt: 0,
            frame: [0.0; FRAME],
            frame_len: 0,
            smooth: 0.0,
            onset: 0,
            in_speech: false,
            preroll: VecDeque::with_capacity(PREROLL_FRAMES),
            buf: Vec::new(),
            voiced_ms: 0,
            silence_ms: 0,
            level_max: 0.0,
            level_frames: 0,
        }
    }

    fn push(&mut self, input: &[f32], ctx: &Ctx) {
        for &s in input {
            self.acc += s;
            self.cnt += 1;
            self.phase += 1.0;
            let mut first = true;
            while self.phase >= self.ratio {
                self.phase -= self.ratio;
                let v = if first { self.acc / self.cnt.max(1) as f32 } else { s };
                first = false;
                self.push_sample(v, ctx);
            }
            if !first {
                self.acc = 0.0;
                self.cnt = 0;
            }
        }
    }

    fn push_silence(&mut self, ms: u32, ctx: &Ctx) {
        for _ in 0..(RATE * ms / 1000) {
            self.push_sample(0.0, ctx);
        }
    }

    fn push_sample(&mut self, v: f32, ctx: &Ctx) {
        self.frame[self.frame_len] = v;
        self.frame_len += 1;
        if self.frame_len == FRAME {
            self.frame_len = 0;
            self.on_frame(self.frame, ctx);
        }
    }

    fn reset(&mut self) {
        self.in_speech = false;
        self.onset = 0;
        self.buf.clear();
        self.preroll.clear();
        self.voiced_ms = 0;
        self.silence_ms = 0;
    }

    fn on_frame(&mut self, frame: [f32; FRAME], ctx: &Ctx) {
        let rms = (frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32).sqrt();
        // fast attack, slower release
        let k = if rms > self.smooth { 0.5 } else { 0.15 };
        self.smooth += (rms - self.smooth) * k;

        self.level_max = self.level_max.max(self.smooth);
        self.level_frames += 1;
        if self.level_frames >= 10 {
            let _ = ctx.app.emit(
                "level",
                serde_json::json!({ "source": ctx.src.name(), "level": self.level_max }),
            );
            self.level_max = 0.0;
            self.level_frames = 0;
        }

        if ctx.shared.paused.load(Ordering::Relaxed) {
            self.reset();
            return;
        }
        let (thr, pause_ms, min_ms, max_samples) = {
            let s = ctx.shared.settings.lock().unwrap();
            (s.vad_threshold, s.pause_ms, s.min_utterance_ms, s.max_utterance_s as usize * RATE as usize)
        };
        let voiced = self.smooth > thr;

        if !self.in_speech {
            if self.preroll.len() == PREROLL_FRAMES {
                self.preroll.pop_front();
            }
            self.preroll.push_back(frame);
            self.onset = if voiced { self.onset + 1 } else { 0 };
            if self.onset >= ONSET_FRAMES {
                self.in_speech = true;
                self.buf.clear();
                self.buf.extend(self.preroll.drain(..).flatten());
                self.voiced_ms = self.onset * FRAME_MS;
                self.silence_ms = 0;
            }
            return;
        }

        self.buf.extend_from_slice(&frame);
        if voiced {
            self.voiced_ms += FRAME_MS;
            self.silence_ms = 0;
        } else {
            self.silence_ms += FRAME_MS;
        }
        if self.silence_ms >= pause_ms {
            let excess = (self.silence_ms.saturating_sub(TAIL_KEEP_MS) * RATE / 1000) as usize;
            let keep = self.buf.len().saturating_sub(excess);
            self.buf.truncate(keep);
            self.finish(min_ms, ctx);
            self.reset();
        } else if self.buf.len() >= max_samples {
            // forced cut of an overly long utterance; keep listening
            self.finish(min_ms, ctx);
            self.buf.clear();
            self.voiced_ms = 0;
        }
    }

    fn finish(&mut self, min_ms: u32, ctx: &Ctx) {
        if self.voiced_ms < min_ms || self.buf.is_empty() {
            return;
        }
        #[cfg(debug_assertions)]
        eprintln!(
            "[vad] {:?} utterance {:.2}s (voiced {:.2}s)",
            ctx.src,
            self.buf.len() as f32 / RATE as f32,
            self.voiced_ms as f32 / 1000.0
        );
        let peak = self.buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let gain = if peak > 0.0 && peak < 0.5 { (0.9 / peak).min(20.0) } else { 1.0 };
        let pcm = self
            .buf
            .iter()
            .map(|x| ((x * gain).clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect();
        // A bounded broadcast channel evicts the oldest queued utterance on overflow.
        if ctx.tx.send(Arc::new(Utterance { pcm, queued_at: Instant::now() })).is_err() {
            eprintln!("[audio] transcription pipeline is closed; dropping utterance");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{mono_chunks, AUDIO_CHUNK_SAMPLES};

    #[test]
    fn stereo_downmix_preserves_order_across_chunk_boundary_and_partial_tail() {
        let input: Vec<f32> = (0..AUDIO_CHUNK_SAMPLES + 3)
            .flat_map(|i| [i as f32, i as f32 + 2.0])
            .collect();
        let chunks: Vec<_> = mono_chunks(&input, 2).collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len, AUDIO_CHUNK_SAMPLES);
        assert_eq!(chunks[1].len, 3);
        let mono: Vec<_> = chunks.iter().flat_map(|c| c.samples[..c.len].iter().copied()).collect();
        assert_eq!(mono, (0..AUDIO_CHUNK_SAMPLES + 3).map(|i| i as f32 + 1.0).collect::<Vec<_>>());
    }

    #[test]
    fn integer_audio_is_normalized_and_empty_input_produces_no_chunks() {
        let chunks: Vec<_> = mono_chunks(&[i16::MIN, 0, 16384], 1).collect();
        assert_eq!(&chunks[0].samples[..chunks[0].len], &[-1.0, 0.0, 0.5]);
        assert_eq!(mono_chunks::<f32>(&[], 2).count(), 0);
    }
}
