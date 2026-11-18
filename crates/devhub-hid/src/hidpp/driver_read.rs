//! HID++ オンボードプロファイルの読み出し手順 (どのセクタを読むか) と
//! `DeviceState` への変換。書き込みはしない。
//!
//! 手順は libratbag `hidpp20.c` `hidpp20_onboard_profiles_initialize` に従う:
//! 1. ディレクトリ (sector `0x0000` = `HIDPP20_USER_PROFILES_G402`, hidpp20.c:1810) を読む。
//! 2. CRC が正しければエントリ i の address (通常 `0x0000 | (i + 1)`) を読む。
//! 3. ディレクトリの CRC が壊れている (工場出荷時) か、ユーザープロファイルの CRC が
//!    壊れていれば ROM プロファイル `0x0100 + i + 1` (`HIDPP20_ROM_PROFILES_G402`,
//!    hidpp20.c:1811, 2866-2868) を CRC 検査なしで読む。ROM 数 (`profile_count_oob`) を
//!    超える i は ROM の先頭 `0x0101` を使う。

use super::onboard_io::OnboardProfilesDesc;
use super::proto::Hidpp;
use crate::hidpp_onboard::crc::crc_ccitt;
use crate::hidpp_onboard::{parse_directory, profile, OnboardProfile, ProfileDirectory};
use devhub_core::backend::HubResult;
use devhub_core::model::*;
use std::collections::BTreeMap;

/// プロファイルディレクトリのセクタ (hidpp20.c:1810 `HIDPP20_USER_PROFILES_G402`)。
pub const DIRECTORY_SECTOR: u16 = 0x0000;
/// ROM プロファイルの基点 (hidpp20.c:1811 `HIDPP20_ROM_PROFILES_G402`)。
pub const ROM_PROFILES_BASE: u16 = 0x0100;
/// ポーリング設定の id (有線機。mousefit.rs / demo.rs と同じ)。
pub const SETTING_POLLING: &str = "polling_wired";

/// プロファイルをどこから読んだか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileSource {
    /// ディレクトリが指すユーザープロファイル (CRC 正)。
    User,
    /// ROM (工場出荷時の既定値)。
    Rom,
}

/// 読み出したプロファイル 1 個と、その生セクタ。
#[derive(Debug)]
pub struct ProfileRead {
    pub source: ProfileSource,
    pub sector: u16,
    /// セクタの生バイト (診断ダンプ用)。
    pub bytes: Vec<u8>,
    pub profile: OnboardProfile,
}

/// ディレクトリセクタを読む。CRC が壊れていれば 2 つ目は None (ROM を使う合図)。
pub fn read_directory(
    hp: &Hidpp,
    desc: &OnboardProfilesDesc,
) -> HubResult<(Vec<u8>, Option<ProfileDirectory>)> {
    let size = desc.sector_size;
    let bytes = hp.read_sector(DIRECTORY_SECTOR, size)?;
    let dir = parse_directory(&bytes, size as usize).ok();
    Ok((bytes, dir))
}

/// ROM プロファイルのセクタ (hidpp20.c:2865-2868)。
pub fn rom_sector(index: u8, rom_count: u8) -> u16 {
    if index >= rom_count {
        ROM_PROFILES_BASE + 1
    } else {
        ROM_PROFILES_BASE + u16::from(index) + 1
    }
}

/// プロファイル `index` (0 始まり) を読む。ユーザー側が使えなければ ROM に落ちる。
pub fn read_profile(
    hp: &Hidpp,
    desc: &OnboardProfilesDesc,
    dir: Option<&ProfileDirectory>,
    index: u8,
) -> HubResult<ProfileRead> {
    let size = desc.sector_size;
    let buttons = usize::from(desc.button_count).min(profile::MAX_BUTTONS);
    if let Some(entry) = dir.and_then(|d| d.entries.get(usize::from(index))) {
        let bytes = hp.read_sector(entry.address, size)?;
        if let Ok(p) = profile::parse(&bytes, size as usize, buttons) {
            return Ok(ProfileRead {
                source: ProfileSource::User,
                sector: entry.address,
                bytes,
                profile: p,
            });
        }
    }
    let sector = rom_sector(index, desc.profile_count_oob);
    let bytes = hp.read_sector(sector, size)?;
    let p = parse_unchecked(&bytes, size as usize, buttons)?;
    Ok(ProfileRead {
        source: ProfileSource::Rom,
        sector,
        bytes,
        profile: p,
    })
}

/// CRC を検査せずに parse する (libratbag は ROM を `check_crc = false` で読む: hidpp20.c:2870-2873)。
/// `profile::parse` は CRC を必ず検査するので、写しの末尾 CRC を付け直して渡す。
fn parse_unchecked(bytes: &[u8], size: usize, buttons: usize) -> HubResult<OnboardProfile> {
    let mut fixed = bytes.to_vec();
    if size >= 2 && fixed.len() == size {
        let crc = crc_ccitt(&fixed[..size - 2]);
        fixed[size - 2..].copy_from_slice(&crc.to_be_bytes());
    }
    profile::parse(&fixed, size, buttons)
}

/// プロファイル + 現在のセンサー DPI → `DeviceState`。
///
/// ステージは DPI 0 (無効) を除いた並び。現在ステージは現在 DPI と一致する段、
/// 無ければプロファイルの既定段。
pub fn build_state(
    p: &OnboardProfile,
    active_profile: u8,
    current_dpi: Option<u16>,
    with_polling: bool,
) -> DeviceState {
    let enabled: Vec<(usize, u16)> = p
        .dpi
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, d)| *d != 0)
        .collect();
    let stages = enabled
        .iter()
        .map(|(_, d)| DpiStage {
            x: u32::from(*d),
            y: None,
            color: Rgb::new(0, 0, 0),
            enabled: true,
        })
        .collect::<Vec<_>>();
    let by_dpi = current_dpi.and_then(|c| enabled.iter().position(|(_, d)| *d == c));
    let by_default = enabled
        .iter()
        .position(|(slot, _)| *slot == usize::from(p.default_dpi_index));
    let active_stage = by_dpi.or(by_default).unwrap_or(0);

    let mut settings = BTreeMap::new();
    if with_polling {
        settings.insert(
            SETTING_POLLING.into(),
            SettingValue::Choice(i64::from(p.report_rate_hz)),
        );
    }
    DeviceState {
        profile: ProfileState {
            active: active_profile,
            names: vec![],
        },
        settings,
        mouse: Some(MouseState {
            stages,
            active_stage,
            buttons: vec![],
            macros: vec![],
            adaptive: None,
        }),
        keyboard: None,
        lighting: BTreeMap::new(),
        peq: None,
    }
}
