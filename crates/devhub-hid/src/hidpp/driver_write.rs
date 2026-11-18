//! HID++ オンボードプロファイルへの書き込み (現在プロファイルのセクタ 1 個だけ)。
//!
//! 手順: getOnboardMode (1=onboard 以外は拒否・切り替えない) → getCurrentProfile →
//! ディレクトリ → プロファイルを読む → ROM なら拒否 (ディレクトリは作らない) →
//! 読んだ構造体を編集 → `profile::serialize` (CRC) → `write_sector` → 読み戻して全バイト照合。
//!
//! 安全策: 編集前の構造体を serialize したものが読んだバイトと一致しない (= 往復で
//! 変わる領域がある) ときは書かない。これで「編集した箇所 + CRC」以外は必ず元のまま。

use super::driver_buttons::action_to_binding;
use super::driver_read::{read_active_profile, ProfileSource, SETTING_POLLING};
use super::onboard_io::OnboardProfilesDesc;
use super::proto::Hidpp;
use crate::hidpp_onboard::buttons::ButtonBinding;
use crate::hidpp_onboard::{profile, OnboardProfile};
use devhub_core::backend::{HubError, HubResult};
use devhub_core::model::*;

/// getOnboardMode の「オンボード」(hidpp20.c:1797-1800)。
pub const ONBOARD_MODE_ONBOARD: u8 = 1;
/// マウスボタン 1 (左) のマスク。
const LEFT_MASK: u16 = 1;
/// 読み戻し不一致のエラーに載せる差分オフセットの上限。
const DIFF_REPORT_MAX: usize = 8;

/// 現在プロファイルを `edit` で書き換えて同じセクタへ書き、読み戻して照合する。
/// `edit` が Err を返したら何も書かない。
pub fn write_active_profile(
    hp: &Hidpp,
    desc: &OnboardProfilesDesc,
    edit: impl FnOnce(&mut OnboardProfile) -> HubResult<()>,
) -> HubResult<()> {
    let mode = hp
        .onboard_mode()?
        .ok_or(HubError::Unsupported("OnboardProfiles (0x8100)"))?;
    if mode != ONBOARD_MODE_ONBOARD {
        return Err(HubError::Unsupported("オンボードモード以外 (ホストモード) への書き込み"));
    }
    let (_, read) = read_active_profile(hp, desc)?;
    if read.source == ProfileSource::Rom {
        return Err(HubError::Unsupported("ROM プロファイル"));
    }
    let size = usize::from(desc.sector_size);
    if profile::serialize(&read.profile, size) != read.bytes {
        return Err(HubError::Protocol(format!(
            "sector {:#06x} の再エンコードが読み出しと一致しないため書き込みを中止しました",
            read.sector
        )));
    }
    let mut edited = read.profile;
    edit(&mut edited)?;
    let bytes = profile::serialize(&edited, size);
    hp.write_sector(read.sector, &bytes)?;
    let back = hp.read_sector(read.sector, desc.sector_size)?;
    let diff = diff_offsets(&bytes, &back);
    if !diff.is_empty() {
        let shown: Vec<_> = diff.iter().take(DIFF_REPORT_MAX).collect();
        return Err(HubError::Protocol(format!(
            "読み戻し不一致: sector {:#06x} の {} バイト (offset {shown:?})",
            read.sector,
            diff.len()
        )));
    }
    Ok(())
}

/// 2 つのバイト列で値が異なる offset (長さが違えば短い方の末尾以降も差分)。
pub fn diff_offsets(a: &[u8], b: &[u8]) -> Vec<usize> {
    (0..a.len().max(b.len()))
        .filter(|&i| a.get(i) != b.get(i))
        .collect()
}

/// `change` 1 件を構造体へ反映する (I/O なし)。書き込み前の検証もここで行う。
pub fn edit_profile(p: &mut OnboardProfile, change: &Change, caps: &Capabilities) -> HubResult<()> {
    let mouse = caps
        .mouse
        .as_ref()
        .ok_or(HubError::Unsupported("マウス設定"))?;
    match change {
        Change::Button { slot, action } => set_button(p, *slot, action, mouse.require_left_click),
        Change::DpiStages { stages, active } => set_dpi_stages(p, stages, *active, mouse),
        Change::Setting { id, value } if id == SETTING_POLLING => set_polling(p, value, caps),
        _ => Err(HubError::NotImplemented("HID++ オンボードプロファイルへのこの変更")),
    }
}

/// 有効なステージ (DPI 0 を除いた並び) の `index` 番目の DPI。
pub fn stage_dpi(p: &OnboardProfile, index: usize) -> HubResult<u16> {
    p.dpi
        .iter()
        .copied()
        .filter(|d| *d != 0)
        .nth(index)
        .ok_or_else(|| HubError::Invalid(format!("DPI ステージ {index} は存在しません")))
}

fn set_button(
    p: &mut OnboardProfile,
    slot: usize,
    action: &ButtonAction,
    require_left_click: bool,
) -> HubResult<()> {
    let Some(current) = p.buttons.get(slot).copied() else {
        return Err(HubError::Invalid(format!(
            "ボタン slot {slot} は範囲外です (0..{})",
            p.buttons.len()
        )));
    };
    let next = action_to_binding(action, &current)?;
    if require_left_click {
        let had_left = p.buttons.iter().any(is_left);
        let left_after = p
            .buttons
            .iter()
            .enumerate()
            .any(|(i, b)| is_left(if i == slot { &next } else { b }));
        if had_left && !left_after {
            return Err(HubError::Invalid(
                "左クリックが割り当てられたボタンが無くなるため変更できません".into(),
            ));
        }
    }
    p.buttons[slot] = next;
    Ok(())
}

fn is_left(b: &ButtonBinding) -> bool {
    matches!(b, ButtonBinding::Mouse { mask } if *mask == LEFT_MASK)
}

fn set_dpi_stages(
    p: &mut OnboardProfile,
    stages: &[DpiStage],
    active: usize,
    mouse: &MouseCaps,
) -> HubResult<()> {
    let max = mouse.max_stages.min(profile::DPI_COUNT);
    let min = mouse.min_stages.max(1);
    if stages.len() < min || stages.len() > max {
        return Err(HubError::Invalid(format!(
            "DPI ステージ数 {} は {min}..={max} の範囲外です",
            stages.len()
        )));
    }
    if active >= stages.len() {
        return Err(HubError::Invalid(format!("現在ステージ {active} は範囲外です")));
    }
    let mut dpi = [0u16; profile::DPI_COUNT];
    for (slot, stage) in stages.iter().enumerate() {
        let x = stage.x;
        if x < mouse.dpi_min || x > mouse.dpi_max || x % mouse.dpi_step != 0 {
            return Err(HubError::Invalid(format!(
                "DPI {x} は {}..={} の {} 刻みではありません",
                mouse.dpi_min, mouse.dpi_max, mouse.dpi_step
            )));
        }
        if stage.y.is_some_and(|y| y != x) {
            return Err(HubError::Invalid("XY 別 DPI には対応していません".into()));
        }
        dpi[slot] = u16::try_from(x).map_err(|_| HubError::Invalid(format!("DPI {x} が大きすぎます")))?;
    }
    p.dpi = dpi;
    // active < stages.len() <= DPI_COUNT (5) なので u8 に収まる。
    p.default_dpi_index = active as u8;
    Ok(())
}

fn set_polling(p: &mut OnboardProfile, value: &SettingValue, caps: &Capabilities) -> HubResult<()> {
    let SettingValue::Choice(hz) = value else {
        return Err(HubError::Invalid(format!("ポーリングレートの値が不正です: {value:?}")));
    };
    let offered = caps.setting(SETTING_POLLING).is_some_and(|spec| match &spec.kind {
        SettingKind::Choice { options } => options.iter().any(|o| o.value == *hz),
        _ => false,
    });
    if !offered {
        return Err(HubError::Invalid(format!("ポーリングレート {hz} Hz には対応していません")));
    }
    p.report_rate_hz = u16::try_from(*hz).map_err(|_| HubError::Invalid(format!("{hz} Hz")))?;
    Ok(())
}
