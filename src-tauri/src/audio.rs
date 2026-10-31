//! System-audio (WASAPI loopback on Windows; selected CoreAudio input on macOS)
//! and optional microphone capture with a simple
//! energy-based voice activity detector. Finished utterances (16 kHz mono i16)
//! are sent to the transcription pipeline. Audio never touches the disk.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::UnboundedSender;

use crate::Shared;

pub const RATE: u32 = 16_000;
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
}

pub fn spawn(app: AppHandle, shared: Arc<Shared>, tx: UnboundedSender<Utterance>) {
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
fn capture_loop(src: Source, app: AppHandle, shared: Arc<Shared>, tx: UnboundedSender<Utterance>) {
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
        let (data_tx, data_rx) = mpsc::channel::<Vec<f32>>();
        let failed = Arc::new(AtomicBool::new(false));
        let (stream, rate) = match build_stream(src, &device, data_tx, failed.clone()) {
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
                Ok(buf) => proc.push(&buf, &ctx),
                // Loopback devices may deliver nothing while nothing plays; treat it
                // as silence so a pending utterance still gets finalized.
                Err(mpsc::RecvTimeoutError::Timeout) => proc.push_silence(150, &ctx),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
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
    tx: mpsc::Sender<Vec<f32>>,
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
    let err_fn = move |_e: cpal::StreamError| failed.store(true, Ordering::Relaxed);
    let stream = match format {
        SampleFormat::F32 => build::<f32>(device, &config, channels, tx, err_fn),
        SampleFormat::I16 => build::<i16>(device, &config, channels, tx, err_fn),
        SampleFormat::U16 => build::<u16>(device, &config, channels, tx, err_fn),
        SampleFormat::I32 => build::<i32>(device, &config, channels, tx, err_fn),
        other => return Err(format!("формат {other:?} не поддерживается")),
    }
    .map_err(|e| e.to_string())?;
    Ok((stream, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    tx: mpsc::Sender<Vec<f32>>,
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
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|c| c.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / c.len() as f32)
                .collect();
            let _ = tx.send(mono);
        },
        err_fn,
        None,
    )
}

struct Ctx<'a> {
    src: Source,
    app: &'a AppHandle,
    shared: &'a Shared,
    tx: &'a UnboundedSender<Utterance>,
}

struct Processor {
    // integrate-and-dump resampler to 16 kHz
    ratio: f64,
    phase: f64,
    acc: f32,
    cnt: u32,
    frame: Vec<f32>,
    // VAD
    smooth: f32,
    onset: u32,
    in_speech: bool,
    preroll: VecDeque<Vec<f32>>,
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
            frame: Vec::with_capacity(FRAME),
            smooth: 0.0,
            onset: 0,
            in_speech: false,
            preroll: VecDeque::new(),
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
        self.frame.push(v);
        if self.frame.len() == FRAME {
            let f = std::mem::replace(&mut self.frame, Vec::with_capacity(FRAME));
            self.on_frame(f, ctx);
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

    fn on_frame(&mut self, frame: Vec<f32>, ctx: &Ctx) {
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
            self.preroll.push_back(frame);
            if self.preroll.len() > PREROLL_FRAMES {
                self.preroll.pop_front();
            }
            self.onset = if voiced { self.onset + 1 } else { 0 };
            if self.onset >= ONSET_FRAMES {
                self.in_speech = true;
                self.buf = self.preroll.drain(..).flatten().collect();
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
        let _ = ctx.tx.send(Utterance { pcm });
    }
}
