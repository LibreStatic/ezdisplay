//! System audio capture through WASAPI loopback of the default render device.
//!
//! Known limitation: loopback taps the mix, it does not redirect it, so the PC keeps playing the
//! audio locally while it is streamed.
//!
//! `start` spawns the capture thread `ezd-audio-wasapi`, which owns all COM objects (it joins the
//! MTA), opens the default render endpoint in shared mode with the loopback flag and reads
//! packets into a bounded channel that `next_samples` drains. Nothing touches COM before `start`.
//! If the device goes away (`AUDCLNT_E_DEVICE_INVALIDATED`, default device switched, ...) the
//! thread ends and `next_samples` reports the error; the session then stops the audio only.

use crate::audio_math::{self, WaveFormat};
use ezd_host_core::{AudioBlock, AudioSource, SourceError, now_us};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    WAVEFORMATEXTENSIBLE_0, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::GUID;

/// Blocks queued between the capture thread and `next_samples` (about 1 s at 20 ms); newer
/// blocks are dropped when the consumer is behind.
const QUEUE_BLOCKS: usize = 50;
/// How long the capture thread waits for the buffer event before it polls anyway (loopback
/// delivers no events while nothing plays on some Windows versions) and checks the stop flag.
const WAIT_MS: u32 = 100;
/// How long `start` waits for the capture thread to open the device.
const INIT_TIMEOUT: Duration = Duration::from_secs(3);
/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`.
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

type Failure = Arc<Mutex<Option<String>>>;

/// WASAPI loopback [`AudioSource`].
pub struct WasapiLoopback {
    rx: Option<Receiver<AudioBlock>>,
    stop: Arc<AtomicBool>,
    failure: Failure,
    thread: Option<JoinHandle<()>>,
}

impl WasapiLoopback {
    pub fn new() -> Self {
        Self {
            rx: None,
            stop: Arc::new(AtomicBool::new(false)),
            failure: Arc::new(Mutex::new(None)),
            thread: None,
        }
    }
}

impl Default for WasapiLoopback {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSource for WasapiLoopback {
    fn start(&mut self, channels: u8) -> Result<(), SourceError> {
        self.stop();
        self.stop = Arc::new(AtomicBool::new(false));
        self.failure = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::sync_channel::<AudioBlock>(QUEUE_BLOCKS);
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let stop = self.stop.clone();
        let failure = self.failure.clone();
        let thread = std::thread::Builder::new()
            .name("ezd-audio-wasapi".into())
            .spawn(move || capture_thread(channels, tx, init_tx, stop, failure))
            .map_err(SourceError::backend)?;
        self.thread = Some(thread);
        match init_rx.recv_timeout(INIT_TIMEOUT) {
            Ok(Ok(())) => {
                self.rx = Some(rx);
                Ok(())
            }
            Ok(Err(e)) => {
                self.stop();
                Err(SourceError::msg(e))
            }
            Err(_) => {
                // Do not join a thread stuck in a driver call; it exits on the stop flag.
                self.stop.store(true, Ordering::SeqCst);
                self.thread = None;
                Err(SourceError::msg("WASAPI loopback did not start in time"))
            }
        }
    }

    fn next_samples(&mut self, timeout: Duration) -> Result<Option<AudioBlock>, SourceError> {
        let Some(rx) = &self.rx else {
            return Err(SourceError::msg("audio capture not started"));
        };
        match rx.recv_timeout(timeout) {
            Ok(b) => Ok(Some(b)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                let e = self.failure.lock().ok().and_then(|mut f| f.take());
                Err(SourceError::msg(
                    e.unwrap_or_else(|| "WASAPI capture stopped".into()),
                ))
            }
        }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.rx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for WasapiLoopback {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Closes the buffer event on every exit path.
struct EventGuard(HANDLE);
impl Drop for EventGuard {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `CreateEventW` and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Balances `CoInitializeEx` on every exit path.
struct ComGuard;
impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: only constructed after a successful `CoInitializeEx` on this thread.
        unsafe { CoUninitialize() }
    }
}

fn capture_thread(
    channels: u8,
    tx: mpsc::SyncSender<AudioBlock>,
    init_tx: mpsc::SyncSender<Result<(), String>>,
    stop: Arc<AtomicBool>,
    failure: Failure,
) {
    // SAFETY: COM is initialised on this thread and released by `ComGuard` after all COM objects
    // created inside `run` are dropped.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() {
        let _ = init_tx.send(Err(format!("CoInitializeEx failed: {hr:?}")));
        return;
    }
    let _com = ComGuard;
    let fmt = audio_math::wave_format(channels);
    if let Err(e) = run(&fmt, &tx, &init_tx, &stop) {
        tracing::warn!("WASAPI loopback ended: {e}");
        // Reports a failure that happened before `init_tx` was answered, too.
        let _ = init_tx.try_send(Err(e.clone()));
        if let Ok(mut f) = failure.lock() {
            *f = Some(e);
        }
    }
}

fn run(
    fmt: &WaveFormat,
    tx: &mpsc::SyncSender<AudioBlock>,
    init_tx: &mpsc::SyncSender<Result<(), String>>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let err = |what: &str, e: windows::core::Error| format!("{what}: {e}");
    // SAFETY: plain COM calls on objects owned by this function; pointers passed to
    // `Initialize`/`GetBuffer` outlive the calls, and the packet slice is only used before
    // `ReleaseBuffer`.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| err("creating the device enumerator", e))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| err("no default audio output device", e))?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| err("activating the audio client", e))?;

        let wf = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: audio_math::FORMAT_TAG_EXTENSIBLE,
                nChannels: fmt.channels,
                nSamplesPerSec: fmt.sample_rate,
                nAvgBytesPerSec: fmt.avg_bytes_per_sec,
                nBlockAlign: fmt.block_align,
                wBitsPerSample: fmt.bits_per_sample,
                cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: fmt.bits_per_sample,
            },
            dwChannelMask: fmt.channel_mask,
            SubFormat: SUBTYPE_IEEE_FLOAT,
        };
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                audio_math::buffer_duration_hns(),
                0,
                (&wf as *const WAVEFORMATEXTENSIBLE).cast::<WAVEFORMATEX>(),
                None,
            )
            .map_err(|e| err("initialising loopback capture", e))?;

        let event = CreateEventW(None, false, false, None)
            .map_err(|e| err("creating the buffer event", e))?;
        let _event = EventGuard(event);
        client
            .SetEventHandle(event)
            .map_err(|e| err("setting the buffer event", e))?;
        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| err("getting the capture client", e))?;
        client.Start().map_err(|e| err("starting capture", e))?;
        tracing::info!(
            channels = fmt.channels,
            "WASAPI loopback capture started (default output device)"
        );
        let _ = init_tx.send(Ok(()));

        let mut result = Ok(());
        'outer: while !stop.load(Ordering::SeqCst) {
            // Event or timeout: either way drain whatever is queued.
            let _ = WaitForSingleObject(event, WAIT_MS);
            loop {
                if stop.load(Ordering::SeqCst) {
                    break 'outer;
                }
                match capture.GetNextPacketSize() {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => {
                        result = Err(err("reading the packet size", e));
                        break 'outer;
                    }
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut buf_flags = 0u32;
                if let Err(e) =
                    capture.GetBuffer(&mut data, &mut frames, &mut buf_flags, None, None)
                {
                    result = Err(err("reading a packet", e));
                    break 'outer;
                }
                let read_us = now_us();
                let raw: &[u8] = if data.is_null() || frames == 0 {
                    &[]
                } else {
                    std::slice::from_raw_parts(data, frames as usize * usize::from(fmt.block_align))
                };
                let samples = audio_math::packet_samples(raw, frames, fmt.channels, buf_flags);
                let _ = capture.ReleaseBuffer(frames);
                if frames == 0 {
                    break;
                }
                let block = AudioBlock {
                    pts_us: audio_math::packet_pts_us(read_us, frames),
                    samples,
                };
                match tx.try_send(block) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => break 'outer,
                }
            }
        }
        let _ = client.Stop();
        result
    }
}
