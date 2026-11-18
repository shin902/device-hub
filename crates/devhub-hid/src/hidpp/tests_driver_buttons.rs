//! `driver_buttons` の変換テスト (実機ダンプ・Special 系)。
//! probe3.log の全プロファイルのボタン割当を round-trip で確認する。

use super::driver_buttons::{action_to_binding, binding_to_action};
use crate::hidpp_onboard::buttons::{ButtonBinding, SpecialAction};
use crate::hidpp_onboard::profile::{parse, DPI_COUNT};
use crate::hidpp_onboard::OnboardProfile;
use devhub_core::model::*;

const SECTOR: usize = 256;
const BUTTONS: usize = 11;

/// セクタを自作して `parse` で構造体を作る。
/// (`name_raw` は private なので parse 経由でしか構築できない。)
pub(super) fn profile_with(dpi: [u16; DPI_COUNT], shift_dpi_index: u8) -> OnboardProfile {
    let mut b = vec![0xFF_u8; SECTOR];
    b[0] = 0x01; // 1000 Hz
    b[1] = 0;
    b[2] = shift_dpi_index;
    for (i, v) in dpi.iter().enumerate() {
        b[3 + 2 * i..5 + 2 * i].copy_from_slice(&v.to_le_bytes());
    }
    b[13..32].fill(0);
    b[32..32 + 4 * BUTTONS].fill(0xFF);
    let crc = crate::hidpp_onboard::crc::crc_ccitt(&b[..SECTOR - 2]);
    b[SECTOR - 2..].copy_from_slice(&crc.to_be_bytes());
    parse(&b, SECTOR, BUTTONS).unwrap()
}

pub(super) fn base_profile() -> OnboardProfile {
    profile_with([1200, 2400, 3200, 6400, 0], 0)
}

pub(super) fn sp(action: SpecialAction) -> ButtonBinding {
    ButtonBinding::Special { action, reserved: 0xff, profile: 0xff }
}

/// probe3.log の全 5 プロファイル × 11 ボタンの割当 (実機確認済み)。
fn probe3_bindings() -> Vec<Vec<ButtonBinding>> {
    let mouse = |mask: u16| ButtonBinding::Mouse { mask };
    let default11 = || vec![
        mouse(1),
        mouse(2),
        mouse(4),
        mouse(8),
        mouse(16),
        sp(SpecialAction::DpiShift),
        sp(SpecialAction::DpiDown),
        sp(SpecialAction::DpiUp),
        sp(SpecialAction::ProfileCycle),
        sp(SpecialAction::WheelRight),
        sp(SpecialAction::WheelLeft),
    ];
    let mut entries = vec![default11(), default11(), default11(), default11(), default11()];
    // entry 2 (sector 0x0003): G6 = GShift、G7 = Ctrl+F、G8 = Ctrl+0x19。
    entries[2][5] = sp(SpecialAction::GShift);
    entries[2][6] = ButtonBinding::Keyboard { modifiers: 1, key: 0x06 };
    entries[2][7] = ButtonBinding::Keyboard { modifiers: 1, key: 0x19 };
    entries
}

#[test]
fn probe3_bindings_roundtrip_through_actions() {
    let p = base_profile();
    for (pi, bindings) in probe3_bindings().iter().enumerate() {
        for (slot, b) in bindings.iter().enumerate() {
            let action = binding_to_action(b, &p);
            let back = action_to_binding(&action, b).unwrap();
            assert_eq!(back, *b, "profile {pi} slot {slot}");
        }
    }
}

#[test]
fn gshift_becomes_unsupported_and_roundtrips() {
    let p = base_profile();
    let b = sp(SpecialAction::GShift);
    let action = binding_to_action(&b, &p);
    let ButtonAction::Unsupported { ty, params } = &action else {
        panic!("GShift は Unsupported になるはず");
    };
    assert_eq!(*ty, 0x90);
    assert_eq!(params, &vec![0x0b, 0xff, 0xff]);
    assert_eq!(action_to_binding(&action, &b).unwrap(), b);
}

#[test]
fn wheel_and_scroll_specials_map_to_mousefn_both_ways() {
    let p = base_profile();
    let cases = [
        (SpecialAction::WheelLeft, MouseFn::ScrollLeft),
        (SpecialAction::WheelRight, MouseFn::ScrollRight),
        (SpecialAction::ScrollUp, MouseFn::ScrollUp),
        (SpecialAction::ScrollDown, MouseFn::ScrollDown),
    ];
    for (action, f) in cases {
        let b = sp(action);
        assert_eq!(binding_to_action(&b, &p), ButtonAction::Mouse(f), "{action:?}");
        assert_eq!(action_to_binding(&ButtonAction::Mouse(f), &b).unwrap(), b, "{f:?}");
    }
}

#[test]
fn wheel_special_write_keeps_reserved_and_profile_bytes() {
    let current = ButtonBinding::Special {
        action: SpecialAction::WheelRight,
        reserved: 0x42,
        profile: 0x07,
    };
    // 別のホイール Special への変更でも reserved/profile は引き継ぐ。
    let next = action_to_binding(&ButtonAction::Mouse(MouseFn::ScrollUp), &current).unwrap();
    assert_eq!(
        next,
        ButtonBinding::Special { action: SpecialAction::ScrollUp, reserved: 0x42, profile: 0x07 }
    );
    // current が Special 以外のときは 0xFF 2 つ (実機の実測値)。
    let fresh = action_to_binding(
        &ButtonAction::Mouse(MouseFn::ScrollLeft),
        &ButtonBinding::Mouse { mask: 1 },
    )
    .unwrap();
    assert_eq!(fresh, sp(SpecialAction::WheelLeft));
}

#[test]
fn dpishift_maps_to_shift_dpi_from_profile() {
    let p = profile_with([800, 1600, 0, 0, 0], 1);
    let b = sp(SpecialAction::DpiShift);
    assert_eq!(binding_to_action(&b, &p), ButtonAction::Dpi(DpiFn::Shift(1600)));
    // 書き込みは値を無視して DpiShift を出す。
    let back = action_to_binding(&ButtonAction::Dpi(DpiFn::Shift(9999)), &b).unwrap();
    assert_eq!(back, b);
}

#[test]
fn dpishift_out_of_range_index_is_zero() {
    let p = profile_with([800, 0, 0, 0, 0], 4); // dpi[4] = 0
    let b = sp(SpecialAction::DpiShift);
    assert_eq!(binding_to_action(&b, &p), ButtonAction::Dpi(DpiFn::Shift(0)));
}

#[test]
fn special_write_keeps_reserved_and_profile_bytes() {
    let p = base_profile();
    // 未対応の特殊割当 (BatteryIndicator) → Unsupported 経由で元バイトを保存。
    let current = ButtonBinding::Special {
        action: SpecialAction::BatteryIndicator,
        reserved: 0x42,
        profile: 0x07,
    };
    let action = binding_to_action(&current, &p);
    assert!(matches!(action, ButtonAction::Unsupported { .. }));
    assert_eq!(action_to_binding(&action, &current).unwrap(), current);

    // 対応済みの Special への変更は reserved/profile を引き継ぐ。
    let next = action_to_binding(&ButtonAction::Dpi(DpiFn::Up), &current).unwrap();
    assert_eq!(
        next,
        ButtonBinding::Special { action: SpecialAction::DpiUp, reserved: 0x42, profile: 0x07 }
    );
    // current が Special でないときは 0xFF 2 つ (実機の実測値)。
    let fresh = action_to_binding(&ButtonAction::ProfileCycle, &ButtonBinding::Mouse { mask: 1 }).unwrap();
    assert_eq!(fresh, sp(SpecialAction::ProfileCycle));
}

#[test]
fn disabled_keeps_tail_bytes() {
    let p = base_profile();
    let current = ButtonBinding::Disabled { tail: [0xff, 0x00, 0x00] };
    assert_eq!(binding_to_action(&current, &p), ButtonAction::Disabled);
    // Disabled 同士は tail を引き継ぐ。
    assert_eq!(action_to_binding(&ButtonAction::Disabled, &current).unwrap(), current);
    // 他の種類から Disabled へは 0xFF 3 つ。
    let fresh = action_to_binding(&ButtonAction::Disabled, &ButtonBinding::Mouse { mask: 1 }).unwrap();
    assert_eq!(fresh, ButtonBinding::Disabled { tail: [0xff, 0xff, 0xff] });
}
