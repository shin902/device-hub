//! `driver_buttons` の変換テスト (キーボード・メディア・caps)。
//! 共有ヘルパは `tests_driver_buttons` から借りる。

use super::driver_buttons::{action_to_binding, binding_to_action, slot_name, BUTTON_KINDS};
use super::tests_driver_buttons::base_profile;
use crate::hidpp_onboard::buttons::ButtonBinding;
use devhub_core::backend::HubError;
use devhub_core::model::*;

#[test]
fn keyboard_maps_to_keystroke_and_back() {
    let p = base_profile();
    let b = ButtonBinding::Keyboard { modifiers: 0x05, key: 0x06 }; // Ctrl+Shift+F
    let action = binding_to_action(&b, &p);
    assert_eq!(action, ButtonAction::Key(KeyStroke { mods: 0x05, usage: 0x06 }));
    assert_eq!(action_to_binding(&action, &b).unwrap(), b);
    // 修飾のみ (usage 0)。
    let b0 = ButtonBinding::Keyboard { modifiers: 0x08, key: 0x00 };
    let a0 = binding_to_action(&b0, &p);
    assert_eq!(a0, ButtonAction::Key(KeyStroke { mods: 0x08, usage: 0 }));
    assert_eq!(action_to_binding(&a0, &b0).unwrap(), b0);
}

#[test]
fn keyboard_usage_over_0xff_is_rejected() {
    let current = ButtonBinding::Keyboard { modifiers: 0, key: 0x04 };
    let err = action_to_binding(&ButtonAction::Key(KeyStroke { mods: 0, usage: 0x100 }), &current)
        .unwrap_err();
    assert!(matches!(err, HubError::Invalid(_)));
}

#[test]
fn consumer_maps_media_both_ways() {
    let p = base_profile();
    let cases = [
        (0xCD, MediaFn::PlayPause),
        (0xB7, MediaFn::Stop),
        (0xB5, MediaFn::Next),
        (0xB6, MediaFn::Prev),
        (0xE9, MediaFn::VolumeUp),
        (0xEA, MediaFn::VolumeDown),
        (0xE2, MediaFn::Mute),
        (0x192, MediaFn::Calculator),
        (0x194, MediaFn::MyComputer),
        (0x196, MediaFn::Browser),
        (0x18A, MediaFn::Mail),
        (0x6F, MediaFn::BrightnessUp),
        (0x70, MediaFn::BrightnessDown),
        (0x7A, MediaFn::Refresh),
    ];
    for (code, f) in cases {
        let b = ButtonBinding::Consumer { code };
        assert_eq!(binding_to_action(&b, &p), ButtonAction::Media(f), "code 0x{code:X}");
        assert_eq!(action_to_binding(&ButtonAction::Media(f), &b).unwrap(), b);
    }
}

#[test]
fn unlisted_consumer_code_is_unsupported() {
    let p = base_profile();
    let b = ButtonBinding::Consumer { code: 0x0111 };
    let action = binding_to_action(&b, &p);
    assert!(matches!(action, ButtonAction::Unsupported { .. }));
    assert_eq!(action_to_binding(&action, &b).unwrap(), b);
}

#[test]
fn macro_and_raw_bindings_go_unsupported() {
    let p = base_profile();
    for b in [
        ButtonBinding::Macro { page: 3, offset: 12 },
        ButtonBinding::Raw([0x05, 1, 2, 3]),
    ] {
        let action = binding_to_action(&b, &p);
        assert!(matches!(action, ButtonAction::Unsupported { .. }), "{b:?}");
        assert_eq!(action_to_binding(&action, &b).unwrap(), b);
    }
}

#[test]
fn unrepresentable_actions_are_rejected() {
    let current = ButtonBinding::Mouse { mask: 1 };
    for a in [
        ButtonAction::Fire { button: MouseFn::Left, count: 3, interval_ms: 40 },
        ButtonAction::Macro { slot: 0, run: MacroRun::Times(1) },
        ButtonAction::PollingCycle,
        ButtonAction::ScrollModeToggle,
        ButtonAction::Mouse(MouseFn::DoubleClick),
    ] {
        assert!(action_to_binding(&a, &current).is_err(), "{a:?}");
    }
}

#[test]
fn slot_names_follow_sector_order() {
    // TASK 固定: 11 ボタンのとき。
    let names: Vec<String> = (0..11).map(|i| slot_name(i, 11)).collect();
    assert_eq!(names[0], "G1 左クリック");
    assert_eq!(names[5], "G6 スナイパー");
    assert_eq!(names[9], "ホイール右チルト");
    assert_eq!(names[10], "ホイール左チルト");
    // 11 でないときは汎用名。
    assert_eq!(slot_name(0, 10), "ボタン 1");
    assert_eq!(slot_name(3, 16), "ボタン 4");
}

#[test]
fn button_kinds_exclude_unoffered_actions() {
    assert_eq!(BUTTON_KINDS.len(), 6);
    for offered in [
        ButtonActionKind::Disabled,
        ButtonActionKind::Mouse,
        ButtonActionKind::Dpi,
        ButtonActionKind::Key,
        ButtonActionKind::Media,
        ButtonActionKind::ProfileCycle,
    ] {
        assert!(BUTTON_KINDS.contains(&offered));
    }
    for absent in [
        ButtonActionKind::Fire,
        ButtonActionKind::Macro,
        ButtonActionKind::PollingCycle,
        ButtonActionKind::ScrollModeToggle,
    ] {
        assert!(!BUTTON_KINDS.contains(&absent));
    }
}
