//! オーディオ連動 — WASAPI ループバックで再生音を取り、
//! Goertzel でバンドレベル (0..1) に分解する。`livefx::FxMode::Audio` の入力。
//!
//! キャプチャは専用スレッドで走り、最新のバンド列を共有スロットに置く。
//! 取り出しは `bands()` (Mutex のコピー)。サンプルはモノラル化して
//! 1024 点ごとに 15 バンド (60Hz..16kHz 対数配分) を計算し、
//! アタック速め・リリース遅めに平滑する。

use std::sync::{Mutex, OnceLock};

/// バンド数 (キーボードの横 15 列に対応)。
pub const NBANDS: usize = 15;
/// Goertzel の窓幅 (48kHz で約 21ms)。
#[cfg(windows)]
const N: usize = 1024;

static BANDS: OnceLock<Mutex<Vec<f32>>> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();

/// 現在のバンドレベル列 (無音・未開始なら 0 列)。
pub fn bands() -> Vec<f32> {
    BANDS
        .get()
        .map(|m| m.lock().map(|b| b.clone()).unwrap_or_default())
        .unwrap_or_default()
}

/// キャプチャスレッドを一度だけ起動する (失敗しても安全に無音)。
pub fn start() {
    STARTED.get_or_init(|| {
        BANDS.set(Mutex::new(vec![0.; NBANDS])).ok();
        std::thread::Builder::new()
            .name("devhub-audio".into())
            .spawn(|| unsafe { run() })
            .ok();
    });
}

#[cfg(windows)]
fn put(v: &[f32]) {
    if let Some(m) = BANDS.get() {
        if let Ok(mut b) = m.lock() {
            *b = v.to_vec();
        }
    }
}

/// バンド中心周波数 (60..16000Hz を対数配分)。
#[cfg(windows)]
fn band_freqs() -> [f32; NBANDS] {
    let mut f = [0f32; NBANDS];
    let lo = 60f32.ln();
    let hi = 16000f32.ln();
    for (i, f) in f.iter_mut().enumerate() {
        *f = (lo + (hi - lo) * i as f32 / (NBANDS - 1) as f32).exp();
    }
    f
}

/// Goertzel — buf の中の freq 成分の正規化強度 (0..1 付近)。
#[cfg(windows)]
fn goertzel(buf: &[f32], sr: f32, freq: f32) -> f32 {
    let w = 2. * std::f32::consts::PI * freq / sr;
    let cw = 2. * w.cos();
    let (mut s1, mut s2) = (0f32, 0f32);
    for &x in buf {
        let s0 = x + cw * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let p = (s1 * s1 + s2 * s2 - cw * s1 * s2).sqrt() / (N as f32 * 0.5);
    // 対数っぽい持ち上げ + クリップ。
    (p * 2.4).powf(0.6).clamp(0., 1.)
}

#[cfg(windows)]
unsafe fn run() {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
        WAVEFORMATEXTENSIBLE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
        return;
    }
    let en: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
        Ok(e) => e,
        Err(_) => return,
    };
    let dev = match en.GetDefaultAudioEndpoint(eRender, eConsole) {
        Ok(d) => d,
        Err(_) => return,
    };
    let cli: IAudioClient = match dev.Activate(CLSCTX_ALL, None) {
        Ok(c) => c,
        Err(_) => return,
    };
    let fmt = match cli.GetMixFormat() {
        Ok(f) => f,
        Err(_) => return,
    };
    let wfx = &*fmt;
    let sr = wfx.nSamplesPerSec as f32;
    // float32 か (PCM16 は 0x1、float は 0x3 / WAVE_FORMAT_EXTENSIBLE 0xfffe)。
    let tag = wfx.wFormatTag;
    let is_float = if tag == 0x3 {
        true
    } else if tag == 0xfffe {
        let ext = &*(fmt as *const WAVEFORMATEXTENSIBLE);
        ext.SubFormat.data1 == 3
    } else {
        false
    };
    let ch = wfx.nChannels.max(1) as usize;
    let bpframe = wfx.nBlockAlign as usize;
    if !is_float || bpframe == 0 {
        CoTaskMemFree(Some(fmt as *const _));
        return;
    }
    if cli
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            0,
            0,
            fmt,
            None,
        )
        .is_err()
    {
        CoTaskMemFree(Some(fmt as *const _));
        return;
    }
    CoTaskMemFree(Some(fmt as *const _));
    let cap: IAudioCaptureClient = match cli.GetService() {
        Ok(c) => c,
        Err(_) => return,
    };
    if cli.Start().is_err() {
        return;
    }
    let freqs = band_freqs();
    let mut acc: Vec<f32> = Vec::with_capacity(N * 2);
    let mut smooth = vec![0f32; NBANDS];
    // 各バンドの最近ピーク — 小さい音でも振り切る自動正規化。
    let mut peaks = vec![0.05f32; NBANDS];
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let mut pkt = match cap.GetNextPacketSize() {
            Ok(p) => p,
            Err(_) => break,
        };
        while pkt > 0 {
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            if cap
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                .is_err()
            {
                return;
            }
            if !data.is_null() && frames > 0 {
                let n = frames as usize * bpframe;
                let raw = std::slice::from_raw_parts(data as *const u8, n);
                for fr in raw.chunks_exact(bpframe) {
                    // float32 × ch をモノラル化。
                    let mut s = 0f32;
                    for c in 0..ch {
                        s += f32::from_le_bytes([
                            fr[c * 4],
                            fr[c * 4 + 1],
                            fr[c * 4 + 2],
                            fr[c * 4 + 3],
                        ]);
                    }
                    acc.push(s / ch as f32);
                }
            }
            let _ = cap.ReleaseBuffer(frames);
            pkt = cap.GetNextPacketSize().unwrap_or(0);
        }
        while acc.len() >= N {
            let buf: Vec<f32> = acc.drain(..N).collect();
            for (i, &f) in freqs.iter().enumerate() {
                let v = goertzel(&buf, sr, f);
                // ピーク追跡: 立ち上がり即・減衰は約7秒で半分、床は 0.03。
                peaks[i] = (peaks[i] * 0.995).max(v).max(0.03);
                let v = (v / peaks[i]).min(1.);
                // アタック 0.55 / リリース 0.88 の平滑。
                smooth[i] = if v > smooth[i] {
                    smooth[i] * 0.45 + v * 0.55
                } else {
                    smooth[i] * 0.88 + v * 0.12
                };
            }
            put(&smooth);
        }
    }
}

#[cfg(not(windows))]
unsafe fn run() {}
