//! Logitech HID++ 2.0 マウス (G502 HERO 有線 046D:C08B) の `DeviceDriver`。
//!
//! 設定の保存先はオンボードプロファイル (0x8100)。`apply` はボタン・DPI ステージ・
//! ポーリングを現在プロファイルのセクタへ書き (`driver_write`)、現在ステージの切替だけは
//! setSensorDpi で行う (プロファイルは書かない)。
//! オンボードモード (hidpp20.c:1797-1800: 0=変更なし / 1=onboard / 2=host) は
//! libratbag のように切り替えたりせず、onboard (1) でなければ書き込みを拒否する。

use super::driver_buttons;
use super::driver_read::{self, SETTING_POLLING};
use super::driver_read::ProfileRead;
use super::driver_write::{self, OnIdentical, WriteOutcome};
use super::features::DpiList;
use super::onboard_io::OnboardProfilesDesc;
use super::proto::{Hidpp, DEV_IDX_WIRED};
use crate::transport::Channel;
use devhub_core::backend::*;
use devhub_core::model::*;
use std::sync::Mutex;

/// long report (0x11) の長さ (report ID 込み 20B)。
const LONG_REPORT_LEN: usize = 20;
/// AdjustableDPI のセンサー番号 (G502 HERO は 1 個)。
const SENSOR: u8 = 0;
/// DPI リストが値の列挙だったときの刻み (TASK 固定設計)。
const DPI_STEP_FOR_VALUES: u32 = 50;
/// UI に出すポーリングレート (Hz)。デバイスの対応レートとの共通部分だけ出す。
const POLLING_HZ: [u16; 4] = [125, 250, 500, 1000];

pub struct HidppDriver {
    hp: Hidpp,
    /// open 時に FW 文字列まで確定した情報 (`info()` は HID 往復しない)。
    info: DeviceInfo,
    desc: OnboardProfilesDesc,
    caps: Capabilities,
    /// 現在プロファイルのセクタのキャッシュ。getCurrentProfile が別の index を返したとき・
    /// 空のとき・書き込み失敗後だけ読み直す。書き込み成功時は読み戻した検証済みバイトで更新する。
    /// ディレクトリは書かないので、index が変わったときに一緒に読み直すだけで足りる。
    cache: Mutex<Option<CachedProfile>>,
}

/// キャッシュ 1 件: 0 始まりの profile index とそのセクタ。
struct CachedProfile {
    index: u8,
    read: ProfileRead,
}

/// 実機を開く (有線なので device index 0xFF)。
pub fn open(dev: hidapi::HidDevice, info: DeviceInfo) -> HubResult<HidppDriver> {
    let ch = Channel::open(dev, LONG_REPORT_LEN, LONG_REPORT_LEN);
    open_with(Hidpp::new(ch, DEV_IDX_WIRED), info)
}

/// プロトコル層から作る (テストはモック Channel で呼ぶ)。
/// オンボードプロファイル記述・DPI リスト・レポートレート一覧を読んで caps を確定し、
/// FW 文字列を読んで `info` に入れる (読めなければ None のまま。open は失敗させない)。
pub(crate) fn open_with(hp: Hidpp, mut info: DeviceInfo) -> HubResult<HidppDriver> {
    let desc = hp
        .onboard_profiles_desc()?
        .ok_or(HubError::Unsupported("OnboardProfiles (0x8100)"))?;
    let dpi = hp
        .sensor_dpi_list(SENSOR)?
        .ok_or(HubError::Unsupported("AdjustableDPI (0x2201)"))?;
    let rates_ms = hp.report_rate_list()?.unwrap_or_default();
    let caps = build_caps(&dpi, &rates_ms, desc.button_count)?;
    if let Ok(Some(fw)) = hp.main_firmware_string() {
        info.firmware = Some(fw);
    }
    info.supported = true;
    Ok(HidppDriver {
        hp,
        info,
        desc,
        caps,
        cache: Mutex::new(None),
    })
}

/// DPI リストとレポートレート (ms の列) から caps を作る。
fn build_caps(dpi: &DpiList, rates_ms: &[u8], button_count: u8) -> HubResult<Capabilities> {
    let (dpi_min, dpi_max, dpi_step) = match dpi {
        DpiList::Range { min, max, step } => (*min, *max, u32::from(*step)),
        DpiList::Values(v) => {
            let (Some(min), Some(max)) = (v.iter().min(), v.iter().max()) else {
                return Err(HubError::Protocol("DPI リストが空です".into()));
            };
            (*min, *max, DPI_STEP_FOR_VALUES)
        }
    };
    let options: Vec<ChoiceOpt> = POLLING_HZ
        .iter()
        .filter(|hz| rates_ms.iter().any(|ms| u16::from(*ms) * **hz == 1000))
        .map(|hz| ChoiceOpt::new(i64::from(*hz), polling_label(*hz)))
        .collect();
    let mut settings = vec![];
    if !options.is_empty() {
        settings.push(
            SettingSpec::choice(
                SETTING_POLLING,
                section::PERFORMANCE,
                "ポーリングレート",
                options,
            )
            .help("マウスがPCへ位置を報告する頻度 (Hz)"),
        );
    }
    let buttons = (0..usize::from(button_count))
        .map(|i| ButtonSlot { name: driver_buttons::slot_name(i, usize::from(button_count)) })
        .collect::<Vec<_>>();
    Ok(Capabilities {
        settings,
        mouse: Some(MouseCaps {
            dpi_min: u32::from(dpi_min),
            dpi_max: u32::from(dpi_max),
            dpi_step: dpi_step.max(1),
            min_stages: 1,
            max_stages: crate::hidpp_onboard::profile::DPI_COUNT,
            xy_split: false,
            stage_colors: false,
            stage_toggle: false,
            buttons,
            button_kinds: driver_buttons::BUTTON_KINDS.to_vec(),
            require_left_click: true,
            macros: None,
            live_buttons: vec![],
            adaptive_dpi: None,
        }),
        ..Capabilities::empty()
    })
}

fn polling_label(hz: u16) -> String {
    if hz == 1000 {
        "1K".into()
    } else {
        hz.to_string()
    }
}

impl HidppDriver {
    /// テスト用: モック Channel に応答を積むためのプロトコル層。
    #[cfg(test)]
    pub(crate) fn hp_for_test(&self) -> &Hidpp {
        &self.hp
    }

    /// これまでの HID++ 要求数 (`hidppprobe --timing` 用)。
    pub fn request_count(&self) -> u32 {
        self.hp.request_count()
    }

    fn lock_cache(&self) -> std::sync::MutexGuard<'_, Option<CachedProfile>> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 現在プロファイル (index, セクタ)。getCurrentProfile は毎回送り、
    /// キャッシュと index が同じならセクタは読まない。
    fn active_read(&self) -> HubResult<(u8, ProfileRead)> {
        let index = driver_read::current_profile_index(&self.hp, &self.desc)?;
        let mut cache = self.lock_cache();
        if let Some(c) = cache.as_ref().filter(|c| c.index == index) {
            return Ok((index, c.read.clone()));
        }
        let (_, dir) = driver_read::read_directory(&self.hp, &self.desc)?;
        let read = driver_read::read_profile(&self.hp, &self.desc, dir.as_ref(), index)?;
        *cache = Some(CachedProfile { index, read: read.clone() });
        Ok((index, read))
    }

    /// 現在プロファイルへ `change` を書く (編集 → 同一なら省略 → 書き → 読み戻し照合)。
    fn write_profile(&self, change: &Change) -> HubResult<()> {
        let outcome = driver_write::write_active_profile(
            &self.hp,
            &self.desc,
            || self.active_read(),
            |p| driver_write::edit_profile(p, change, &self.caps),
            OnIdentical::Skip,
        );
        match outcome {
            Ok(WriteOutcome::Written { index, read }) => {
                *self.lock_cache() = Some(CachedProfile { index, read: *read });
                Ok(())
            }
            Ok(WriteOutcome::Unchanged) => Ok(()),
            Err(e) => {
                // 途中で失敗したらデバイス側の中身が分からないので次回読み直す。
                *self.lock_cache() = None;
                Err(e)
            }
        }
    }
}

impl DeviceDriver for HidppDriver {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn caps(&self) -> Capabilities {
        self.caps.clone()
    }

    fn read(&self) -> HubResult<DeviceState> {
        let (active, read) = self.active_read()?;
        let current_dpi = self.hp.sensor_dpi(SENSOR).ok().flatten().map(|(d, _)| d);
        let with_polling = self.caps.setting(SETTING_POLLING).is_some();
        Ok(driver_read::build_state(
            &read.profile,
            active,
            current_dpi,
            with_polling,
        ))
    }

    fn apply(&self, change: &Change) -> HubResult<()> {
        match change {
            Change::ActiveStage(index) => {
                let (_, read) = self.active_read()?;
                let dpi = driver_write::stage_dpi(&read.profile, *index)?;
                self.hp.set_sensor_dpi(SENSOR, dpi)
            }
            Change::Button { .. } | Change::DpiStages { .. } => self.write_profile(change),
            Change::Setting { id, .. } if id == SETTING_POLLING => self.write_profile(change),
            _ => Err(HubError::NotImplemented(
                "HID++: ボタン・DPI・ポーリング以外の書き込み",
            )),
        }
    }
}
