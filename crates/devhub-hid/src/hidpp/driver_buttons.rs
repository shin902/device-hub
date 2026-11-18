//! HID++ オンボードの `ButtonBinding` ⇔ 契約の `ButtonAction` 変換。
//!
//! ワイヤ形式は libratbag src/hidpp20.c (MIT) `union hidpp20_button_binding`。
//! UI は caps (`button_kinds`) だけを見るので、ここで解釈できる割当だけ
//! 種類として宣言し、解釈できないものは `Unsupported` で表示・往復保存する。
//!
//! 修飾キー: HID++ の `modifier_flags` は HID の modifier byte と同じビット並び
//! (hidpp20.h:606-621、bit0 LCtrl … bit7 RGUI)。契約の `KeyStroke::mods`
//! (`devhub_core::model::mods`) も同じ並びなので変換不要。

use crate::hidpp_onboard::buttons::{ButtonBinding, SpecialAction};
use crate::hidpp_onboard::OnboardProfile;
use devhub_core::backend::{HubError, HubResult};
use devhub_core::model::*;

/// UI に出す割当種類。マクロ (マクロ領域は未解釈) と Fire (ワイヤ形式に
/// 該当なし) は含めない。
pub const BUTTON_KINDS: [ButtonActionKind; 6] = [
    ButtonActionKind::Disabled,
    ButtonActionKind::Mouse,
    ButtonActionKind::Dpi,
    ButtonActionKind::Key,
    ButtonActionKind::Media,
    ButtonActionKind::ProfileCycle,
];

/// slot 番号 → 表示名。G502 HERO のセクタ内並び (実機確認)。
/// 11 ボタンで無い機種は汎用名に落とす。
pub fn slot_name(index: usize, button_count: usize) -> String {
    const G502: [&str; 11] = [
        "G1 左クリック",
        "G2 右クリック",
        "G3 ホイールクリック",
        "G4 戻る",
        "G5 進む",
        "G6 スナイパー",
        "G7 DPI ダウン",
        "G8 DPI アップ",
        "G9 プロファイル切替",
        "ホイール右チルト",
        "ホイール左チルト",
    ];
    if button_count == G502.len() && index < G502.len() {
        G502[index].into()
    } else {
        format!("ボタン {}", index + 1)
    }
}

/// ワイヤ → 契約。`p` は DpiShift の表示 DPI 算出に使う。
pub fn binding_to_action(b: &ButtonBinding, p: &OnboardProfile) -> ButtonAction {
    match *b {
        ButtonBinding::Mouse { mask } => mouse_action(mask).unwrap_or_else(|| unsupported(b)),
        ButtonBinding::Keyboard { modifiers, key } => {
            ButtonAction::Key(KeyStroke { mods: modifiers, usage: u16::from(key) })
        }
        ButtonBinding::Consumer { code } => media_action(code).unwrap_or_else(|| unsupported(b)),
        ButtonBinding::Special { action, .. } => special_action(action, p).unwrap_or_else(|| unsupported(b)),
        // マクロ領域は未解釈。元のバイトを保持して破壊しない。
        ButtonBinding::Macro { .. } | ButtonBinding::Raw(_) => unsupported(b),
        ButtonBinding::Disabled { .. } => ButtonAction::Disabled,
    }
}

/// 契約 → ワイヤ。`current` は既存割当 (未対応種類の往復保存と
/// Special/Disabled の予備・3 バイト目を引き継ぐのに使う)。
pub fn action_to_binding(a: &ButtonAction, current: &ButtonBinding) -> HubResult<ButtonBinding> {
    Ok(match a {
        ButtonAction::Disabled => {
            let tail = match current {
                ButtonBinding::Disabled { tail } => *tail,
                _ => [0xff, 0xff, 0xff],
            };
            ButtonBinding::Disabled { tail }
        }
        ButtonAction::Mouse(f) => match f {
            // ホイール系は Special (0x90) 経由。current が Special なら
            // reserved/profile を引き継ぐ (実機は 0xFF 2 つ)。
            MouseFn::ScrollUp | MouseFn::ScrollDown | MouseFn::ScrollLeft | MouseFn::ScrollRight => {
                let action = match f {
                    MouseFn::ScrollUp => SpecialAction::ScrollUp,
                    MouseFn::ScrollDown => SpecialAction::ScrollDown,
                    MouseFn::ScrollLeft => SpecialAction::WheelLeft,
                    _ => SpecialAction::WheelRight,
                };
                let (reserved, profile) = match current {
                    ButtonBinding::Special { reserved, profile, .. } => (*reserved, *profile),
                    _ => (0xff, 0xff),
                };
                ButtonBinding::Special { action, reserved, profile }
            }
            MouseFn::DoubleClick => {
                return Err(HubError::Unsupported("ダブルクリック (HID++ ワイヤ形式に該当なし)"));
            }
            _ => ButtonBinding::Mouse { mask: mouse_mask(*f)? },
        },
        ButtonAction::Key(k) => {
            // usage 0 = 修飾のみ (契約の規約)。> 0xFF は HID++ の 1 バイト key に入らない。
            let Some(key) = u8::try_from(k.usage).ok() else {
                return Err(HubError::Invalid(format!("キー usage 0x{:04X} は 0xFF 超です", k.usage)));
            };
            ButtonBinding::Keyboard { modifiers: k.mods, key }
        }
        ButtonAction::Media(f) => ButtonBinding::Consumer { code: media_code(*f) },
        ButtonAction::Dpi(f) => {
            let (action, reserved, profile) = match (f, current) {
                // 同じ種類への変更はワイヤの 3・4 バイト目を引き継ぐ (実機は 0xFF だが
                // 機種によっては profile 番号を持つので壊さない)。
                (DpiFn::Up, ButtonBinding::Special { reserved, profile, .. }) => (SpecialAction::DpiUp, *reserved, *profile),
                (DpiFn::Down, ButtonBinding::Special { reserved, profile, .. }) => (SpecialAction::DpiDown, *reserved, *profile),
                (DpiFn::Cycle, ButtonBinding::Special { reserved, profile, .. }) => (SpecialAction::DpiCycle, *reserved, *profile),
                // Shift の DPI 値はプロファイルの shift 段なので書き込みでは変えられない。
                (DpiFn::Shift(_), ButtonBinding::Special { reserved, profile, .. }) => (SpecialAction::DpiShift, *reserved, *profile),
                (DpiFn::Up, _) => (SpecialAction::DpiUp, 0xff, 0xff),
                (DpiFn::Down, _) => (SpecialAction::DpiDown, 0xff, 0xff),
                (DpiFn::Cycle, _) => (SpecialAction::DpiCycle, 0xff, 0xff),
                (DpiFn::Shift(_), _) => (SpecialAction::DpiShift, 0xff, 0xff),
            };
            ButtonBinding::Special { action, reserved, profile }
        }
        ButtonAction::ProfileCycle => {
            let (reserved, profile) = match current {
                ButtonBinding::Special { reserved, profile, .. } => (*reserved, *profile),
                _ => (0xff, 0xff),
            };
            ButtonBinding::Special { action: SpecialAction::ProfileCycle, reserved, profile }
        }
        // UI には出さない (caps で宣言していない)。将来 caps に足すときは
        // ここにワイヤ変換を足すまで。
        ButtonAction::Fire { .. } => {
            return Err(HubError::Unsupported("Fire (HID++ ワイヤ形式に該当なし)"));
        }
        ButtonAction::Macro { .. } => {
            return Err(HubError::Unsupported("マクロ (オンボードマクロ領域は未解釈)"));
        }
        ButtonAction::PollingCycle => {
            return Err(HubError::Unsupported("ポーリングレート順送り"));
        }
        ButtonAction::ScrollModeToggle => {
            return Err(HubError::Unsupported("スクロールモード切替"));
        }
        // 未対応: 読み出し時のバイトをそのまま保存して破壊しない。
        ButtonAction::Unsupported { .. } => *current,
    })
}

fn unsupported(b: &ButtonBinding) -> ButtonAction {
    let bytes = b.to_bytes();
    ButtonAction::Unsupported { ty: bytes[0], params: bytes[1..].to_vec() }
}

fn mouse_mask(f: MouseFn) -> HubResult<u16> {
    // 呼び出し側でホイール系 (Special 経由) と DoubleClick は振り分け済み。
    let bit = match f {
        MouseFn::Left => 0,
        MouseFn::Right => 1,
        MouseFn::Middle => 2,
        MouseFn::Back => 3,
        MouseFn::Forward => 4,
        _ => {
            return Err(HubError::Unsupported(
                "HID++ のマウス割当はボタン 1-5 (Left/Right/Middle/Back/Forward) だけです",
            ));
        }
    };
    Ok(1 << bit)
}

fn mouse_action(mask: u16) -> Option<ButtonAction> {
    let f = match mask {
        1 => MouseFn::Left,
        2 => MouseFn::Right,
        4 => MouseFn::Middle,
        8 => MouseFn::Back,
        16 => MouseFn::Forward,
        _ => return None,
    };
    Some(ButtonAction::Mouse(f))
}

fn special_action(action: SpecialAction, p: &OnboardProfile) -> Option<ButtonAction> {
    let dpi = p
        .dpi
        .get(usize::from(p.shift_dpi_index))
        .copied()
        .unwrap_or(0);
    Some(match action {
        SpecialAction::DpiUp => ButtonAction::Dpi(DpiFn::Up),
        SpecialAction::DpiDown => ButtonAction::Dpi(DpiFn::Down),
        SpecialAction::DpiCycle => ButtonAction::Dpi(DpiFn::Cycle),
        SpecialAction::DpiShift => ButtonAction::Dpi(DpiFn::Shift(u32::from(dpi))),
        SpecialAction::ProfileCycle => ButtonAction::ProfileCycle,
        // ホイール系は Special 経由で表現する (libratbag の 0x10/0x11・0x01/0x02)。
        SpecialAction::ScrollUp => ButtonAction::Mouse(MouseFn::ScrollUp),
        SpecialAction::ScrollDown => ButtonAction::Mouse(MouseFn::ScrollDown),
        SpecialAction::WheelLeft => ButtonAction::Mouse(MouseFn::ScrollLeft),
        SpecialAction::WheelRight => ButtonAction::Mouse(MouseFn::ScrollRight),
        _ => return None,
    })
}

/// MediaFn ⇔ HID Consumer usage (mousefit8.rs と同じ表)。
fn media_code(f: MediaFn) -> u16 {
    match f {
        MediaFn::PlayPause => 0xCD,
        MediaFn::Stop => 0xB7,
        MediaFn::Next => 0xB5,
        MediaFn::Prev => 0xB6,
        MediaFn::VolumeUp => 0xE9,
        MediaFn::VolumeDown => 0xEA,
        MediaFn::Mute => 0xE2,
        MediaFn::Calculator => 0x192,
        MediaFn::MyComputer => 0x194,
        MediaFn::Browser => 0x196,
        MediaFn::Mail => 0x18A,
        MediaFn::BrightnessUp => 0x6F,
        MediaFn::BrightnessDown => 0x70,
        MediaFn::Refresh => 0x7A,
    }
}

fn media_action(code: u16) -> Option<ButtonAction> {
    let f = match code {
        0xCD => MediaFn::PlayPause,
        0xB7 => MediaFn::Stop,
        0xB5 => MediaFn::Next,
        0xB6 => MediaFn::Prev,
        0xE9 => MediaFn::VolumeUp,
        0xEA => MediaFn::VolumeDown,
        0xE2 => MediaFn::Mute,
        0x192 => MediaFn::Calculator,
        0x194 => MediaFn::MyComputer,
        0x196 => MediaFn::Browser,
        0x18A => MediaFn::Mail,
        0x6F => MediaFn::BrightnessUp,
        0x70 => MediaFn::BrightnessDown,
        0x7A => MediaFn::Refresh,
        _ => return None,
    };
    Some(ButtonAction::Media(f))
}
