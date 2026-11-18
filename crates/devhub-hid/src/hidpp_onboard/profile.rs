//! オンボードプロファイル 1 セクタのバイト列 ⇔ 構造体変換。
//!
//! レイアウト (libratbag src/hidpp20.c `union hidpp20_internal_profile`, MIT から導出):
//! 0 レポート間隔 (1000/Hz) / 1 既定 DPI / 2 シフト DPI / 3-12 DPIx5 (LE) /
//! 13-31 電源ブロック (未解釈・保持) / 32-95 ボタン 16x4B / 96-159 G-shift 側 16x4B /
//! 160-207 名前 48B UTF-16LE / 208-229 LEDx2 (11B) / 230-251 未解釈 LED 領域 (保持) /
//! 252-末尾-3 予備 (保持) / 末尾 2B CRC-CCITT (BE)。
//! セクタサイズは引数で受け取る。G502 HERO は 0x8100 getProfilesDescription の 0x0100 = 256 (実機確認)。

use super::buttons::ButtonBinding;
use super::crc;
use devhub_core::backend::{HubError, HubResult};

/// DPI ステージ数 (libratbag `HIDPP20_DPI_COUNT`)。
pub const DPI_COUNT: usize = 5;
/// LED ゾーン数 (ロゴ / DPI 部)。
pub const LED_COUNT: usize = 2;
/// セクタ内のボタン割り当てスロット上限 (本体側・G-shift 側それぞれ)。
pub const MAX_BUTTONS: usize = 16;

const OFFSET_DPI: usize = 3;
const OFFSET_POWER_BLOCK: usize = 13;
const OFFSET_BUTTONS: usize = 32;
const OFFSET_SHIFT_BUTTONS: usize = 96;
const OFFSET_NAME: usize = 160;
const OFFSET_LEDS: usize = 208;
const OFFSET_SHIFT_LEDS: usize = 230;
const OFFSET_TAIL: usize = 252;
const NAME_BYTES: usize = 48;
const POWER_BLOCK_LEN: usize = OFFSET_BUTTONS - OFFSET_POWER_BLOCK;
const SHIFT_LEDS_LEN: usize = OFFSET_TAIL - OFFSET_SHIFT_LEDS;
/// 末尾予備 1B + CRC 2B を含めたレイアウト最小サイズ。
const MIN_SECTOR_SIZE: usize = OFFSET_TAIL + 3;

/// RGB 色 (各 0-255)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

/// LED ゾーン 1 つの設定。ワイヤは 11 バイト固定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedMode {
    Off,
    /// 固定色。
    Fixed { color: Rgb },
    Cycle { period_ms: u16, brightness: u8 },
    Breathing { color: Rgb, period_ms: u16, waveform: u8, brightness: u8 },
    /// 解釈対象外のモード・非正規バイト列。そのまま保持して書き戻す。
    Raw([u8; 11]),
}

/// LED 11 バイトを構造体へ割り当てる。明るさ 0 は仕様上 100% 扱い。
fn decode_led(bytes: &[u8; 11]) -> LedMode {
    let color_at = |at: usize| Rgb { red: bytes[at], green: bytes[at + 1], blue: bytes[at + 2] };
    match bytes[0] {
        0x00 => LedMode::Off,
        0x01 => LedMode::Fixed { color: color_at(1) },
        0x03 => LedMode::Cycle {
            period_ms: u16::from_be_bytes([bytes[6], bytes[7]]),
            brightness: if bytes[8] == 0 { 100 } else { bytes[8] },
        },
        0x0a => LedMode::Breathing {
            color: color_at(1),
            period_ms: u16::from_be_bytes([bytes[4], bytes[5]]),
            waveform: bytes[6],
            brightness: if bytes[7] == 0 { 100 } else { bytes[7] },
        },
        _ => LedMode::Raw(*bytes),
    }
}

/// 構造体を LED 11 バイトへ書き込む。明るさ 100% 以上は仕様上 0 に戻す。
fn encode_led(mode: &LedMode) -> [u8; 11] {
    let mut out = [0u8; 11];
    match mode {
        LedMode::Raw(bytes) => *bytes,
        LedMode::Off => out,
        LedMode::Fixed { color } => {
            out[0] = 0x01;
            out[1..4].copy_from_slice(&[color.red, color.green, color.blue]);
            out
        }
        LedMode::Cycle { period_ms, brightness } => {
            out[0] = 0x03;
            out[6..8].copy_from_slice(&period_ms.to_be_bytes());
            out[8] = (*brightness).min(99);
            out
        }
        LedMode::Breathing { color, period_ms, waveform, brightness } => {
            out[0] = 0x0a;
            out[1..4].copy_from_slice(&[color.red, color.green, color.blue]);
            out[4..6].copy_from_slice(&period_ms.to_be_bytes());
            out[6] = *waveform;
            out[7] = (*brightness).min(99);
            out
        }
    }
}

/// UTF-16LE 名前領域 (48B) を文字列へ。0xFF 埋めのときは空、それ以外は
/// NUL までをデコードする (デコード失敗時は U+FFFD を詰め、壊れた文字は保持)。
fn decode_name(bytes: &[u8]) -> String {
    if bytes.iter().filter(|&&b| b == 0xff).count() == bytes.len() {
        return String::new();
    }
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if unit == 0x0000 || unit == 0xffff {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

/// 文字列を UTF-16LE 48 バイトへ。長すぎる場合は 24 コード単位で切り詰め、
/// 残りを 0xFF で埋める (デバイスの初期値に合わせる)。
fn encode_name(name: &str) -> Vec<u8> {
    let mut units: Vec<u16> = name.encode_utf16().take(NAME_BYTES / 2).collect();
    if units.len() < NAME_BYTES / 2 {
        units.push(0x0000);
    }
    while units.len() < NAME_BYTES / 2 {
        units.push(0xffff);
    }
    units
        .iter()
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

/// オンボードプロファイル 1 つ分。解釈するフィールドは構造体に起こし、
/// 未解釈領域は `spare` に生バイトで保持して serialize で書き戻す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardProfile {
    /// レポートレート (Hz)。ワイヤは 1000/Hz (125→8、1000→1)。
    pub report_rate_hz: u16,
    /// 既定 DPI インデックス (0-4)。
    pub default_dpi_index: u8,
    /// DPI シフト (G-shift 押下時) のインデックス (0-4)。
    pub shift_dpi_index: u8,
    /// DPI 5 段 (0 = 無効)。ワイヤは little-endian。
    pub dpi: [u16; DPI_COUNT],
    /// 本体側ボタン割り当て (ボタン数はデバイスから受ける。G502 HERO は 11)。
    pub buttons: Vec<ButtonBinding>,
    /// G-shift 側ボタン割り当て (`buttons` と同じ長さ)。
    pub shift_buttons: Vec<ButtonBinding>,
    /// プロファイル名 (UTF-16LE 48 バイト = 24 文字分)。
    pub name: String,
    /// 名前領域の生バイト (48B1B)。名前が未設定 (0xFF 埋め) の場合に備えて保持する。
    name_raw: [u8; NAME_BYTES],
    /// LED (0 = ロゴ、1 = DPI 部)。G-shift 側領域には libratbag にならい本体側と同じ内容を書く。
    pub leds: [LedMode; LED_COUNT],
    /// LED 領域の生バイト。`leds` が読み込み時と同じなら serialize はこれをそのまま書く
    /// (明るさ 0=100% や未使用バイトを再エンコードで変えないため)。
    leds_raw: [[u8; 11]; LED_COUNT],
    /// 未解釈領域: 電源ブロック (13-31)・G-shift 側 LED (230-251)・末尾予備 (252-)。
    pub spare: Vec<u8>,
}

/// `buttons` と `shift_buttons` のスロット数を検証する。
fn button_slot_count(buttons: usize, shift_buttons: usize) -> HubResult<usize> {
    if buttons == shift_buttons && buttons <= MAX_BUTTONS {
        Ok(buttons)
    } else {
        Err(HubError::Protocol(format!(
            "ボタン数が不正です: 本体 {buttons} / G-shift {shift_buttons} (上限 {MAX_BUTTONS})"
        )))
    }
}

/// セクタ 1 個分のバイト列をプロファイルへ変換する。
///
/// `bytes` の長さは `sector_size` と一致すること。末尾 2 バイトの CRC-CCITT が
/// 内容と一致しなければ `Err` を返す。
pub fn parse(bytes: &[u8], sector_size: usize, button_count: usize) -> HubResult<OnboardProfile> {
    button_slot_count(button_count, button_count)?;
    if bytes.len() != sector_size || sector_size < MIN_SECTOR_SIZE {
        return Err(HubError::Protocol(format!(
            "セクタサイズが不正です: {sector_size} バイト (len {})",
            bytes.len()
        )));
    }
    let stored = u16::from_be_bytes([bytes[sector_size - 2], bytes[sector_size - 1]]);
    let calculated = crc::crc_ccitt(&bytes[..sector_size - 2]);
    if stored != calculated {
        return Err(HubError::Protocol(format!(
            "CRC 不一致: stored 0x{stored:04x} != calc 0x{calculated:04x}"
        )));
    }
    let report_rate_ms = u16::from(bytes[0]);
    if report_rate_ms == 0 {
        return Err(HubError::Protocol("レポート間隔が 0 です".into()));
    }
    let mut dpi = [0u16; DPI_COUNT];
    for (slot, stage) in dpi.iter_mut().enumerate() {
        *stage = u16::from_le_bytes([bytes[OFFSET_DPI + 2 * slot], bytes[OFFSET_DPI + 2 * slot + 1]]);
    }
    let buttons = (0..button_count)
        .map(|slot| ButtonBinding::from_bytes(&bytes[OFFSET_BUTTONS + 4 * slot..][..4].try_into().unwrap()))
        .collect::<Vec<_>>();
    let shift_buttons = (0..button_count)
        .map(|slot| ButtonBinding::from_bytes(&bytes[OFFSET_SHIFT_BUTTONS + 4 * slot..][..4].try_into().unwrap()))
        .collect::<Vec<_>>();
    let leds_raw: [[u8; 11]; LED_COUNT] = [
        bytes[OFFSET_LEDS..][..11].try_into().unwrap(),
        bytes[OFFSET_LEDS + 11..][..11].try_into().unwrap(),
    ];
    let leds = [decode_led(&leds_raw[0]), decode_led(&leds_raw[1])];
    let mut spare = Vec::with_capacity(sector_size - OFFSET_POWER_BLOCK - 2);
    spare.extend_from_slice(&bytes[OFFSET_POWER_BLOCK..OFFSET_BUTTONS]);
    spare.extend_from_slice(&bytes[OFFSET_SHIFT_LEDS..OFFSET_TAIL]);
    spare.extend_from_slice(&bytes[OFFSET_TAIL..sector_size - 2]);
    Ok(OnboardProfile {
        report_rate_hz: 1000 / report_rate_ms,
        default_dpi_index: bytes[1],
        shift_dpi_index: bytes[2],
        dpi,
        buttons,
        shift_buttons,
        name: decode_name(&bytes[OFFSET_NAME..][..NAME_BYTES]),
        name_raw: bytes[OFFSET_NAME..][..NAME_BYTES].try_into().unwrap(),
        leds,
        leds_raw,
        spare,
    })
}

/// プロファイルをセクタ 1 個分のバイト列へ書き戻す。CRC は付け直す。
///
/// 未解釈領域 (`spare`) は読み込み時のバイトをそのまま書き戻すので、
/// parse → serialize の往復で未知の領域は壊れない。
pub fn serialize(profile: &OnboardProfile, sector_size: usize) -> Vec<u8> {
    // parse 済みの構造体だけを受け付ける (ボタン数の不一致はプログラムバグ)。
    if button_slot_count(profile.buttons.len(), profile.shift_buttons.len()).is_err() {
        panic!("serialize: invalid button counts");
    }
    let mut bytes = vec![0xff_u8; sector_size];
    if sector_size < MIN_SECTOR_SIZE {
        bytes.clear();
        return bytes;
    }
    bytes[0] = (1000 / u32::from(profile.report_rate_hz.max(1))).min(255) as u8;
    bytes[1] = profile.default_dpi_index;
    bytes[2] = profile.shift_dpi_index;
    for (slot, &stage) in profile.dpi.iter().enumerate() {
        bytes[OFFSET_DPI + 2 * slot..OFFSET_DPI + 2 * slot + 2].copy_from_slice(&stage.to_le_bytes());
    }
    for (slot, binding) in profile.buttons.iter().enumerate() {
        bytes[OFFSET_BUTTONS + 4 * slot..OFFSET_BUTTONS + 4 * slot + 4].copy_from_slice(&binding.to_bytes());
    }
    for (slot, binding) in profile.shift_buttons.iter().enumerate() {
        bytes[OFFSET_SHIFT_BUTTONS + 4 * slot..OFFSET_SHIFT_BUTTONS + 4 * slot + 4].copy_from_slice(&binding.to_bytes());
    }
    // 名前が読み込み時のままなら生バイトを書く (0xFF 埋めの未設定名を `00 00 ff..` に変えない)。
    if decode_name(&profile.name_raw) == profile.name {
        bytes[OFFSET_NAME..OFFSET_NAME + NAME_BYTES].copy_from_slice(&profile.name_raw);
    } else {
        bytes[OFFSET_NAME..OFFSET_NAME + NAME_BYTES].copy_from_slice(&encode_name(&profile.name));
    }
    for (zone, led) in profile.leds.iter().enumerate() {
        let raw = &profile.leds_raw[zone];
        let encoded = if decode_led(raw) == *led { *raw } else { encode_led(led) };
        bytes[OFFSET_LEDS + 11 * zone..OFFSET_LEDS + 11 * zone + 11].copy_from_slice(&encoded);
    }
    let mut at = 0;
    bytes[OFFSET_POWER_BLOCK..OFFSET_BUTTONS].copy_from_slice(&profile.spare[at..at + POWER_BLOCK_LEN]);
    at += POWER_BLOCK_LEN;
    bytes[OFFSET_SHIFT_LEDS..OFFSET_TAIL].copy_from_slice(&profile.spare[at..at + SHIFT_LEDS_LEN]);
    at += SHIFT_LEDS_LEN;
    bytes[OFFSET_TAIL..sector_size - 2].copy_from_slice(&profile.spare[at..]);
    let checksum = crc::crc_ccitt(&bytes[..sector_size - 2]);
    bytes[sector_size - 2..sector_size].copy_from_slice(&checksum.to_be_bytes());
    bytes
}


#[cfg(test)]
mod tests {
    use super::{crc, parse, serialize, ButtonBinding, LedMode, Rgb, MAX_BUTTONS};
    use crate::hidpp_onboard::buttons::SpecialAction;

    const SECTOR_SIZE: usize = 255;
    const BUTTON_COUNT: usize = 11;

    fn build_g502_default_sector() -> Vec<u8> {
        let mut bytes = vec![0xff_u8; SECTOR_SIZE];
        bytes[0] = 0x01;
        bytes[1] = 0x02;
        bytes[2] = 0x00;
        for (slot, dpi) in [400u16, 800, 1600, 3200, 6400].iter().enumerate() {
            bytes[3 + 2 * slot..5 + 2 * slot].copy_from_slice(&dpi.to_le_bytes());
        }
        bytes[13..32].fill(0x00);
        let bindings = [
            ButtonBinding::Mouse { mask: 0x0001 },
            ButtonBinding::Mouse { mask: 0x0002 },
            ButtonBinding::Mouse { mask: 0x0004 },
            ButtonBinding::Mouse { mask: 0x0008 },
            ButtonBinding::Mouse { mask: 0x0010 },
            ButtonBinding::Special { action: SpecialAction::DpiDown, reserved: 0, profile: 0 },
            ButtonBinding::Special { action: SpecialAction::DpiUp, reserved: 0, profile: 0 },
            ButtonBinding::Special { action: SpecialAction::ProfileCycle, reserved: 0, profile: 0 },
            ButtonBinding::Special { action: SpecialAction::WheelLeft, reserved: 0, profile: 0 },
            ButtonBinding::Special { action: SpecialAction::WheelRight, reserved: 0, profile: 0 },
            ButtonBinding::Special { action: SpecialAction::GShift, reserved: 0, profile: 0 },
        ];
        for (slot, binding) in bindings.iter().enumerate() {
            bytes[32 + 4 * slot..36 + 4 * slot].copy_from_slice(&binding.to_bytes());
        }
        let mut name_units: Vec<u16> = "G502".encode_utf16().collect();
        name_units.push(0x0000);
        while name_units.len() < 24 {
            name_units.push(0xffff);
        }
        for (unit, value) in name_units.iter().enumerate() {
            bytes[160 + 2 * unit..162 + 2 * unit].copy_from_slice(&value.to_le_bytes());
        }
        bytes[208..219].copy_from_slice(&[0x01, 0x01, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        bytes[219..230].copy_from_slice(&[0x01, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        bytes[230..252].fill(0x00);
        bytes[252] = 0x00;
        let checksum = crc::crc_ccitt(&bytes[..SECTOR_SIZE - 2]);
        bytes[SECTOR_SIZE - 2..SECTOR_SIZE].copy_from_slice(&checksum.to_be_bytes());
        bytes
    }

    #[test]
    fn parse_serialize_roundtrip_preserves_bytes_and_unknown_fields() {
        let sector = build_g502_default_sector();
        let parsed = parse(&sector, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        assert_eq!(parsed.report_rate_hz, 1000);
        assert_eq!(parsed.default_dpi_index, 2);
        assert_eq!(parsed.shift_dpi_index, 0);
        assert_eq!(parsed.dpi, [400, 800, 1600, 3200, 6400]);
        assert_eq!(parsed.name, "G502");
        assert_eq!(parsed.buttons.len(), BUTTON_COUNT);
        assert_eq!(parsed.buttons[0], ButtonBinding::Mouse { mask: 0x0001 });
        let serialized = serialize(&parsed, SECTOR_SIZE);
        assert_eq!(serialized, sector);
    }

    #[test]
    fn changing_dpi_updates_only_that_offset_and_crc() {
        let sector = build_g502_default_sector();
        let mut parsed = parse(&sector, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        parsed.dpi[2] = 2000;
        let serialized = serialize(&parsed, SECTOR_SIZE);
        let reparsed = parse(&serialized, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        assert_eq!(reparsed.dpi, [400, 800, 2000, 3200, 6400]);
        assert_eq!(reparsed.report_rate_hz, parsed.report_rate_hz);
        assert_eq!(reparsed.name, parsed.name);
        let mut expected = sector.clone();
        expected[3 + 2 * 2..5 + 2 * 2].copy_from_slice(&2000u16.to_le_bytes());
        let checksum = crc::crc_ccitt(&expected[..SECTOR_SIZE - 2]);
        expected[SECTOR_SIZE - 2..SECTOR_SIZE].copy_from_slice(&checksum.to_be_bytes());
        assert_eq!(serialized, expected);
    }

    #[test]
    fn changing_report_rate_updates_only_first_byte_and_crc() {
        let sector = build_g502_default_sector();
        let mut parsed = parse(&sector, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        parsed.report_rate_hz = 500;
        let serialized = serialize(&parsed, SECTOR_SIZE);
        assert_eq!(serialized[0], 0x02);
        let reparsed = parse(&serialized, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        assert_eq!(reparsed.report_rate_hz, 500);
    }

    #[test]
    fn changing_led_color_updates_only_led_offset_and_crc() {
        let sector = build_g502_default_sector();
        let mut parsed = parse(&sector, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        parsed.leds[0] = LedMode::Fixed { color: Rgb { red: 0x12, green: 0x34, blue: 0x56 } };
        let serialized = serialize(&parsed, SECTOR_SIZE);
        assert_eq!(serialized[208..212], [0x01, 0x12, 0x34, 0x56]);
        let reparsed = parse(&serialized, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        assert_eq!(reparsed.leds[0], parsed.leds[0]);
    }

    #[test]
    fn changing_one_button_updates_only_that_slot_and_crc() {
        let sector = build_g502_default_sector();
        let mut parsed = parse(&sector, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        parsed.buttons[5] = ButtonBinding::Keyboard { modifiers: 0x01, key: 0x04 };
        let serialized = serialize(&parsed, SECTOR_SIZE);
        assert_eq!(serialized[32 + 4 * 5..36 + 4 * 5], [0x80, 0x02, 0x01, 0x04]);
        let reparsed = parse(&serialized, SECTOR_SIZE, BUTTON_COUNT).unwrap();
        assert_eq!(reparsed.buttons[5], ButtonBinding::Keyboard { modifiers: 0x01, key: 0x04 });
    }

    #[test]
    fn parse_rejects_crc_mismatch() {
        let mut sector = build_g502_default_sector();
        sector[0] = 0x02;
        assert!(parse(&sector, SECTOR_SIZE, BUTTON_COUNT).is_err());
    }

    #[test]
    fn parse_rejects_wrong_length() {
        let sector = build_g502_default_sector();
        let short = &sector[..200];
        assert!(parse(short, SECTOR_SIZE, BUTTON_COUNT).is_err());
        assert!(parse(&sector, 254, BUTTON_COUNT).is_err());
    }

    #[test]
    fn parse_rejects_zero_report_rate() {
        let mut sector = build_g502_default_sector();
        sector[0] = 0x00;
        let checksum = crc::crc_ccitt(&sector[..SECTOR_SIZE - 2]);
        sector[SECTOR_SIZE - 2..SECTOR_SIZE].copy_from_slice(&checksum.to_be_bytes());
        assert!(parse(&sector, SECTOR_SIZE, BUTTON_COUNT).is_err());
    }

    #[test]
    fn parse_rejects_invalid_button_count() {
        let sector = build_g502_default_sector();
        assert!(parse(&sector, SECTOR_SIZE, MAX_BUTTONS + 1).is_err());
    }
}
