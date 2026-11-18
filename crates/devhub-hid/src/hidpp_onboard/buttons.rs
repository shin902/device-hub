//! ボタン割り当て (セクタ内 4 バイト) の変換。libratbag src/hidpp20.c (MIT) の
//! `union hidpp20_button_binding` 相当。
//!
//! ワイヤ形式:
//! - `80 01 mm mm` マウスボタン (big-endian ビットマスク、bit 0 = ボタン 1)
//! - `80 02 mod key` キーボード (HID 修飾キー + HID usage)
//! - `80 03 cc cc` コンシューマ制御 (big-endian)
//! - `90 code rsv profile` 特殊機能
//! - `00 page 00 offset` マクロ (3 バイト目は 0 決め打ち)
//! - `ff xx xx xx` 無効

/// 特殊機能コード。libratbag `hidpp20_profiles_specials` 表 (0x01–0x11) に合わせる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialAction {
    WheelLeft,
    WheelRight,
    DpiUp,
    DpiDown,
    DpiCycle,
    DpiDefault,
    DpiShift,
    ProfileUp,
    ProfileDown,
    ProfileCycle,
    GShift,
    BatteryIndicator,
    EnableProfile,
    PerformanceSwitch,
    Host,
    ScrollDown,
    ScrollUp,
    Unknown(u8),
}

impl SpecialAction {
    /// ワイヤ上の特殊機能コード。
    pub fn code(self) -> u8 {
        match self {
            Self::WheelLeft => 0x01,
            Self::WheelRight => 0x02,
            Self::DpiUp => 0x03,
            Self::DpiDown => 0x04,
            Self::DpiCycle => 0x05,
            Self::DpiDefault => 0x06,
            Self::DpiShift => 0x07,
            Self::ProfileUp => 0x08,
            Self::ProfileDown => 0x09,
            Self::ProfileCycle => 0x0a,
            Self::GShift => 0x0b,
            Self::BatteryIndicator => 0x0c,
            Self::EnableProfile => 0x0d,
            Self::PerformanceSwitch => 0x0e,
            Self::Host => 0x0f,
            Self::ScrollDown => 0x10,
            Self::ScrollUp => 0x11,
            Self::Unknown(code) => code,
        }
    }

    /// ワイヤコードから割り当てる。表に無いコードは `Unknown` で保持する。
    pub fn from_code(code: u8) -> Self {
        match code {
            0x01 => Self::WheelLeft,
            0x02 => Self::WheelRight,
            0x03 => Self::DpiUp,
            0x04 => Self::DpiDown,
            0x05 => Self::DpiCycle,
            0x06 => Self::DpiDefault,
            0x07 => Self::DpiShift,
            0x08 => Self::ProfileUp,
            0x09 => Self::ProfileDown,
            0x0a => Self::ProfileCycle,
            0x0b => Self::GShift,
            0x0c => Self::BatteryIndicator,
            0x0d => Self::EnableProfile,
            0x0e => Self::PerformanceSwitch,
            0x0f => Self::Host,
            0x10 => Self::ScrollDown,
            0x11 => Self::ScrollUp,
            other => Self::Unknown(other),
        }
    }
}

/// ボタン 1 つの割り当て。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonBinding {
    /// マウスボタン。`mask` はワイヤのビットマスクをそのまま保持する (bit 0 = ボタン 1)。
    Mouse { mask: u16 },
    /// キーボードキー (HID 修飾キーフラグ + HID usage)。
    Keyboard { modifiers: u8, key: u8 },
    /// コンシューマ制御コード (HID usage)。
    Consumer { code: u16 },
    /// 特殊機能。`reserved`・`profile` はワイヤの 3・4 バイト目をそのまま書き戻す。
    Special { action: SpecialAction, reserved: u8, profile: u8 },
    /// オンボードマクロ (ページとオフセット)。
    Macro { page: u8, offset: u8 },
    /// 無効。`tail` は 2–4 バイト目をそのまま書き戻す。
    Disabled { tail: [u8; 3] },
    /// 未知の形式。4 バイトをそのまま保持する。
    Raw([u8; 4]),
}

impl ButtonBinding {
    /// セクタ内の 4 バイトから割り当てを割り当てる。正規形でないものは `Raw` に回す。
    pub fn from_bytes(bytes: &[u8; 4]) -> Self {
        match bytes[0] {
            0x80 => match bytes[1] {
                0x01 => {
                    let mask = u16::from_be_bytes([bytes[2], bytes[3]]);
                    // マスク 0 は libratbag の変換 (ffs) が定義しない値なので壊さず保持する。
                    if mask == 0 {
                        Self::Raw(*bytes)
                    } else {
                        Self::Mouse { mask }
                    }
                }
                0x02 => Self::Keyboard { modifiers: bytes[2], key: bytes[3] },
                0x03 => Self::Consumer { code: u16::from_be_bytes([bytes[2], bytes[3]]) },
                _ => Self::Raw(*bytes),
            },
            0x90 => Self::Special {
                action: SpecialAction::from_code(bytes[1]),
                reserved: bytes[2],
                profile: bytes[3],
            },
            // マクロは 3 バイト目が 0 以外なら解釈せず Raw で保持する。
            0x00 if bytes[2] == 0 => Self::Macro { page: bytes[1], offset: bytes[3] },
            0xff => Self::Disabled { tail: [bytes[1], bytes[2], bytes[3]] },
            _ => Self::Raw(*bytes),
        }
    }

    /// セクタへ書き込む 4 バイトへ変換する。
    pub fn to_bytes(self) -> [u8; 4] {
        match self {
            Self::Mouse { mask } => [0x80, 0x01, (mask >> 8) as u8, mask as u8],
            Self::Keyboard { modifiers, key } => [0x80, 0x02, modifiers, key],
            Self::Consumer { code } => [0x80, 0x03, (code >> 8) as u8, code as u8],
            Self::Special { action, reserved, profile } => {
                [0x90, action.code(), reserved, profile]
            }
            Self::Macro { page, offset } => [0x00, page, 0x00, offset],
            Self::Disabled { tail } => [0xff, tail[0], tail[1], tail[2]],
            Self::Raw(bytes) => bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ButtonBinding, SpecialAction};

    #[test]
    fn button_binding_roundtrips_every_variant() {
        let bindings = [
            ButtonBinding::Mouse { mask: 0x0104 },
            ButtonBinding::Keyboard { modifiers: 0x05, key: 0x06 },
            ButtonBinding::Consumer { code: 0x00b5 },
            ButtonBinding::Special { action: SpecialAction::DpiCycle, reserved: 0x00, profile: 0x00 },
            ButtonBinding::Special { action: SpecialAction::Unknown(0x42), reserved: 0xff, profile: 0x02 },
            ButtonBinding::Macro { page: 3, offset: 12 },
            ButtonBinding::Disabled { tail: [0xff, 0x00, 0x00] },
            ButtonBinding::Raw([0x85, 1, 2, 3]),
            ButtonBinding::Raw([0x80, 0x01, 0x00, 0x00]),
        ];
        for binding in bindings {
            assert_eq!(ButtonBinding::from_bytes(&binding.to_bytes()), binding);
        }
    }

    #[test]
    fn special_action_codes_match_libratbag_table() {
        let table = [
            (0x01, SpecialAction::WheelLeft),
            (0x02, SpecialAction::WheelRight),
            (0x03, SpecialAction::DpiUp),
            (0x04, SpecialAction::DpiDown),
            (0x05, SpecialAction::DpiCycle),
            (0x06, SpecialAction::DpiDefault),
            (0x07, SpecialAction::DpiShift),
            (0x08, SpecialAction::ProfileUp),
            (0x09, SpecialAction::ProfileDown),
            (0x0a, SpecialAction::ProfileCycle),
            (0x0b, SpecialAction::GShift),
            (0x0c, SpecialAction::BatteryIndicator),
            (0x0d, SpecialAction::EnableProfile),
            (0x0e, SpecialAction::PerformanceSwitch),
            (0x0f, SpecialAction::Host),
            (0x10, SpecialAction::ScrollDown),
            (0x11, SpecialAction::ScrollUp),
        ];
        for (code, action) in table {
            assert_eq!(SpecialAction::from_code(code), action);
            assert_eq!(action.code(), code);
        }
        assert_eq!(SpecialAction::from_code(0x42), SpecialAction::Unknown(0x42));
        assert_eq!(SpecialAction::Unknown(0x42).code(), 0x42);
    }

    #[test]
    fn noncanonical_macro_falls_back_to_raw() {
        let raw = [0x00, 0x02, 0x01, 0x00];
        assert_eq!(ButtonBinding::from_bytes(&raw), ButtonBinding::Raw(raw));
    }
}
