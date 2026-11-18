//! Logitech HID++ 2.0 マウス (G502 HERO 有線 046D:C08B) の `DeviceDriver`。
//!
//! 設定の保存先はオンボードプロファイル (0x8100)。フェーズ 2a は読み出しだけで、
//! `apply` は全て `NotImplemented` (書き込みはフェーズ 2b)。
//! オンボードモード (hidpp20.c:1797-1800: 0=変更なし / 1=onboard / 2=host) は
//! libratbag のように開いた時点で切り替えたりせず、読むだけにする。

use super::driver_buttons;
use super::driver_read::{self, SETTING_POLLING};
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
    info: Mutex<DeviceInfo>,
    desc: OnboardProfilesDesc,
    caps: Capabilities,
}

/// 実機を開く (有線なので device index 0xFF)。
pub fn open(dev: hidapi::HidDevice, info: DeviceInfo) -> HubResult<HidppDriver> {
    let ch = Channel::open(dev, LONG_REPORT_LEN, LONG_REPORT_LEN);
    open_with(Hidpp::new(ch, DEV_IDX_WIRED), info)
}

/// プロトコル層から作る (テストはモック Channel で呼ぶ)。
/// オンボードプロファイル記述・DPI リスト・レポートレート一覧を読んで caps を確定する。
pub(crate) fn open_with(hp: Hidpp, info: DeviceInfo) -> HubResult<HidppDriver> {
    let desc = hp
        .onboard_profiles_desc()?
        .ok_or(HubError::Unsupported("OnboardProfiles (0x8100)"))?;
    let dpi = hp
        .sensor_dpi_list(SENSOR)?
        .ok_or(HubError::Unsupported("AdjustableDPI (0x2201)"))?;
    let rates_ms = hp.report_rate_list()?.unwrap_or_default();
    let caps = build_caps(&dpi, &rates_ms, desc.button_count)?;
    Ok(HidppDriver {
        hp,
        info: Mutex::new(info),
        desc,
        caps,
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

    /// 現在プロファイル (0 始まり)。デバイスは 1 始まりで返す (`driver_read::profile_index`)。
    fn active_profile(&self) -> HubResult<u8> {
        let raw = self
            .hp
            .current_profile()?
            .ok_or(HubError::Unsupported("OnboardProfiles (0x8100)"))?;
        driver_read::profile_index(raw, self.desc.profile_count)
    }
}

impl DeviceDriver for HidppDriver {
    fn info(&self) -> DeviceInfo {
        let mut i = self.info.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Ok(Some(fw)) = self.hp.main_firmware_string() {
            i.firmware = Some(fw);
        }
        i.supported = true;
        i
    }

    fn caps(&self) -> Capabilities {
        self.caps.clone()
    }

    fn read(&self) -> HubResult<DeviceState> {
        let active = self.active_profile()?;
        let (_, dir) = driver_read::read_directory(&self.hp, &self.desc)?;
        let read = driver_read::read_profile(&self.hp, &self.desc, dir.as_ref(), active)?;
        let current_dpi = self.hp.sensor_dpi(SENSOR).ok().flatten().map(|(d, _)| d);
        let with_polling = self.caps.setting(SETTING_POLLING).is_some();
        Ok(driver_read::build_state(
            &read.profile,
            active,
            current_dpi,
            with_polling,
        ))
    }

    fn apply(&self, _change: &Change) -> HubResult<()> {
        Err(HubError::NotImplemented(
            "HID++ オンボードプロファイルへの書き込み (フェーズ 2b)",
        ))
    }
}
