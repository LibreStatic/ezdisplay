//! System audio capture through WASAPI loopback of the default render device.
//!
//! Loopback taps the mix, it does not redirect it, so to keep the PC quiet while it streams the
//! capture thread mutes the device: loopback still captures a muted device, and before its master
//! volume is applied, so the stream carries full-level audio. The volume slider becomes a control
//! value that is kept in step with the client (`take_volume_change` / `set_volume`); this PC's
//! mute is taken, so the client's mute flag is only echoed back. When capture ends the device gets
//! its volume back and is unmuted. If the user unmutes it during the session without moving the
//! slider (moving it unmutes too, and is undone) it keeps playing locally. The state before the session is saved in the registry first, and
//! [`recover_after_crash`] puts it back if the host died with the PC muted.
//!
//! `start` spawns the capture thread `ezd-audio-wasapi`, which owns all COM objects (it joins the
//! MTA), opens the default render endpoint in shared mode with the loopback flag and reads
//! packets into a bounded channel that `next_samples` drains. Nothing touches COM before `start`.
//! If the device goes away (`AUDCLNT_E_DEVICE_INVALIDATED`, default device switched, ...) the
//! thread ends and `next_samples` reports the error; the session then stops the audio only.

use super::touch_keyboard::{delete_value, read_string, write_string};
use crate::audio_math::{self, SavedOutput, WaveFormat};
use crate::touch_keyboard::OWN_KEY;
use ezd_host_core::{AudioBlock, AudioSource, SourceError, now_us};
use ezd_proto::AudioVolume;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient, IAudioClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    WAVEFORMATEXTENSIBLE_0, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{GUID, HSTRING};

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
/// REG_SZ under [`OWN_KEY`] with the [`SavedOutput`] while a session holds the output device.
const SAVED_OUTPUT_VALUE: &str = "SavedAudioOutput";
/// How often the capture thread reads the volume slider and the mute state of the device.
const VOLUME_POLL: Duration = Duration::from_millis(100);

type Failure = Arc<Mutex<Option<String>>>;

/// Volume handoff between the session and the capture thread.
#[derive(Default)]
struct VolumeState {
    /// A slider move on this PC that the session has not taken yet.
    changed: Option<AudioVolume>,
    /// A volume from the client that the capture thread has not applied yet.
    requested: Option<AudioVolume>,
}

type SharedVolume = Arc<Mutex<VolumeState>>;

fn lock(v: &SharedVolume) -> MutexGuard<'_, VolumeState> {
    v.lock().unwrap_or_else(|e| e.into_inner())
}

/// WASAPI loopback [`AudioSource`].
pub struct WasapiLoopback {
    rx: Option<Receiver<AudioBlock>>,
    stop: Arc<AtomicBool>,
    failure: Failure,
    volume: SharedVolume,
    thread: Option<JoinHandle<()>>,
}

impl WasapiLoopback {
    pub fn new() -> Self {
        Self {
            rx: None,
            stop: Arc::new(AtomicBool::new(false)),
            failure: Arc::new(Mutex::new(None)),
            volume: SharedVolume::default(),
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
        self.volume = SharedVolume::default();
        let (tx, rx) = mpsc::sync_channel::<AudioBlock>(QUEUE_BLOCKS);
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let stop = self.stop.clone();
        let failure = self.failure.clone();
        let volume = self.volume.clone();
        let thread = std::thread::Builder::new()
            .name("ezd-audio-wasapi".into())
            .spawn(move || capture_thread(channels, tx, init_tx, stop, failure, volume))
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

    fn take_volume_change(&mut self) -> Option<AudioVolume> {
        lock(&self.volume).changed.take()
    }

    fn set_volume(&mut self, volume: AudioVolume) {
        lock(&self.volume).requested = Some(volume);
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
    volume: SharedVolume,
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
    if let Err(e) = run(&fmt, &tx, &init_tx, &stop, &volume) {
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
    volume: &SharedVolume,
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
        // Without the guard the audio still streams, the PC just keeps playing it too.
        let mut output = OutputGuard::engage(&device)
            .inspect_err(|e| tracing::warn!("not muting the PC output: {e}"))
            .ok();
        tracing::info!(
            channels = fmt.channels,
            muted = output.as_ref().is_some_and(|o| o.saved.muted_by_us),
            "WASAPI loopback capture started (default output device)"
        );
        let _ = init_tx.send(Ok(()));

        let mut result = Ok(());
        let mut next_poll = Instant::now();
        'outer: while !stop.load(Ordering::SeqCst) {
            // Event or timeout: either way drain whatever is queued.
            let _ = WaitForSingleObject(event, WAIT_MS);
            if let Some(o) = &mut output
                && Instant::now() >= next_poll
            {
                o.poll(volume);
                next_poll = Instant::now() + VOLUME_POLL;
            }
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

/// The output device while a session streams it: muted (unless it already was) and given back
/// its volume and mute state on drop. The state before is saved in the registry first.
struct OutputGuard {
    endpoint: IAudioEndpointVolume,
    saved: SavedOutput,
    /// The slider value last seen or set, to tell the user's moves apart from ours.
    last_volume: f32,
    /// The client's mute flag, sent back with every slider move (this PC's mute is ours).
    client_muted: bool,
}

impl OutputGuard {
    /// Saves the state of `device` and mutes it. Needs COM on the calling thread.
    fn engage(device: &IMMDevice) -> Result<Self, String> {
        // A device left muted by a crashed run is restored first; while that fails its saved
        // state must not be overwritten.
        restore_saved().map_err(|e| format!("an earlier session's output is not restored: {e}"))?;
        let err = |what: &str, e: windows::core::Error| format!("{what}: {e}");
        // SAFETY: COM calls on a live device; the id string is copied before it is freed.
        let (endpoint, device_id) = unsafe {
            let endpoint: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| err("activating the endpoint volume", e))?;
            let id = device
                .GetId()
                .map_err(|e| err("reading the device id", e))?;
            let device_id = id.to_string();
            CoTaskMemFree(Some(id.0 as *const _));
            (endpoint, device_id.map_err(|e| format!("device id: {e}"))?)
        };
        // SAFETY: plain getters on a live interface.
        let (volume, was_muted) = unsafe {
            (
                endpoint
                    .GetMasterVolumeLevelScalar()
                    .map_err(|e| err("reading the volume", e))?,
                endpoint
                    .GetMute()
                    .map_err(|e| err("reading the mute state", e))?
                    .as_bool(),
            )
        };
        let saved = SavedOutput {
            device_id,
            volume,
            muted_by_us: !was_muted,
        };
        // Persist first so a crash cannot leave the PC muted.
        write_string(OWN_KEY, SAVED_OUTPUT_VALUE, &saved.encode())
            .map_err(|e| format!("saving the output state: {e}"))?;
        if saved.muted_by_us {
            // SAFETY: plain setter on a live interface; a null event context is allowed.
            if let Err(e) = unsafe { endpoint.SetMute(true, std::ptr::null()) } {
                let _ = delete_value(OWN_KEY, SAVED_OUTPUT_VALUE);
                return Err(err("muting the output", e));
            }
        }
        Ok(Self {
            endpoint,
            saved,
            last_volume: volume,
            client_muted: false,
        })
    }

    /// Applies a volume from the client and reports a slider move on this PC. The first value
    /// is not reported: the client's volume wins when a session starts.
    fn poll(&mut self, shared: &SharedVolume) {
        let requested = lock(shared).requested.take();
        if let Some(v) = requested {
            self.client_muted = v.muted;
            let level = if v.volume.is_nan() {
                0.0
            } else {
                v.volume.clamp(0.0, 1.0)
            };
            // SAFETY: plain setter on a live interface; a null event context is allowed.
            match unsafe {
                self.endpoint
                    .SetMasterVolumeLevelScalar(level, std::ptr::null())
            } {
                Ok(()) => self.last_volume = level,
                Err(e) => tracing::warn!("setting the PC volume failed: {e}"),
            }
        }
        let mut moved = false;
        // SAFETY: plain getter on a live interface.
        if let Ok(level) = unsafe { self.endpoint.GetMasterVolumeLevelScalar() }
            && (level - self.last_volume).abs() >= AudioVolume::EPSILON
        {
            moved = true;
            self.last_volume = level;
            lock(shared).changed = Some(AudioVolume {
                volume: level,
                muted: self.client_muted,
            });
        }
        // SAFETY: plain getter on a live interface.
        if !self.saved.muted_by_us
            || !matches!(unsafe { self.endpoint.GetMute() }, Ok(m) if !m.as_bool())
        {
            return;
        }
        if moved {
            // Moving the Windows volume slider unmutes the device: that is the user setting the
            // tablet's volume, not asking for local sound.
            // SAFETY: plain setter on a live interface; a null event context is allowed.
            if let Err(e) = unsafe { self.endpoint.SetMute(true, std::ptr::null()) } {
                tracing::warn!("muting the PC output again failed: {e}");
            }
            return;
        }
        tracing::info!("the PC output was unmuted by hand; it plays locally from now on");
        self.saved.muted_by_us = false;
        let _ = write_string(OWN_KEY, SAVED_OUTPUT_VALUE, &self.saved.encode());
    }
}

impl Drop for OutputGuard {
    fn drop(&mut self) {
        match restore(&self.endpoint, &self.saved) {
            Ok(()) => {
                if let Err(e) = delete_value(OWN_KEY, SAVED_OUTPUT_VALUE) {
                    tracing::warn!("cannot clear the saved audio output: {e}");
                }
            }
            // The saved state stays for `recover_after_crash`.
            Err(e) => tracing::warn!("restoring the PC output failed: {e}"),
        }
    }
}

/// Puts the volume and mute state of `saved` back on `endpoint`.
fn restore(endpoint: &IAudioEndpointVolume, saved: &SavedOutput) -> Result<(), String> {
    // SAFETY: plain setters on a live interface; a null event context is allowed.
    unsafe {
        endpoint
            .SetMasterVolumeLevelScalar(saved.volume, std::ptr::null())
            .map_err(|e| format!("setting the volume: {e}"))?;
        if saved.muted_by_us {
            endpoint
                .SetMute(false, std::ptr::null())
                .map_err(|e| format!("unmuting: {e}"))?;
        }
    }
    Ok(())
}

/// Restores the device in the saved state left by a run that did not end cleanly, if any, and
/// clears it. A device that no longer exists is forgotten. Needs COM on the calling thread.
fn restore_saved() -> Result<(), String> {
    let Some(raw) = read_string(OWN_KEY, SAVED_OUTPUT_VALUE)? else {
        return Ok(());
    };
    let Some(saved) = SavedOutput::decode(&raw) else {
        tracing::warn!("ignoring malformed saved audio output {raw:?}");
        return delete_value(OWN_KEY, SAVED_OUTPUT_VALUE);
    };
    // SAFETY: plain COM calls on objects owned by this block.
    let endpoint = unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("creating the device enumerator: {e}"))?;
        match enumerator.GetDevice(&HSTRING::from(saved.device_id.as_str())) {
            Ok(device) => device
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                .map_err(|e| format!("activating the endpoint volume: {e}"))?,
            Err(e) => {
                tracing::info!(
                    "the audio output saved by an unclean exit is gone ({e}); forgetting it"
                );
                return delete_value(OWN_KEY, SAVED_OUTPUT_VALUE);
            }
        }
    };
    restore(&endpoint, &saved)?;
    tracing::info!(
        volume = saved.volume,
        unmuted = saved.muted_by_us,
        "restored the PC audio output after an unclean exit"
    );
    delete_value(OWN_KEY, SAVED_OUTPUT_VALUE)
}

/// Puts back the PC audio output if a host that crashed during a session left it muted. Call
/// once when the host starts. Runs on its own thread, which joins the MTA whatever COM mode the
/// caller uses.
pub fn recover_after_crash() {
    let worker = std::thread::Builder::new()
        .name("ezd-audio-recover".into())
        .spawn(|| {
            // SAFETY: COM is released by `ComGuard` after `restore_saved` dropped its objects.
            if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
                return;
            }
            let _com = ComGuard;
            if let Err(e) = restore_saved() {
                tracing::warn!("audio output: restore after an unclean exit failed: {e}");
            }
        });
    if let Ok(w) = worker {
        let _ = w.join();
    }
}
