//! `driver` / `driver_read` のモック Channel テスト。
//! モックは応答を積んだ順に返すので、ドライバの要求順どおりに積む。

use super::driver::{open_with, HidppDriver};
use super::driver_read::SETTING_POLLING;
use super::proto::Hidpp;
use crate::hidpp_onboard::crc::crc_ccitt;
use crate::hidpp_onboard::{serialize_directory, DirectoryEntry, ProfileDirectory};
use crate::transport::Channel;
use devhub_core::backend::DeviceDriver;
use devhub_core::model::*;

pub(super) const IDX: u8 = 0xFF;
/// getFeature で返す feature index。
pub(super) const F_ONB: u8 = 0x09;
pub(super) const F_DPI: u8 = 0x07;
pub(super) const F_RATE: u8 = 0x08;
/// G502 HERO 実機の getProfilesDescription と同じ 256B。
pub(super) const SECTOR: usize = 256;
/// G502 HERO 風: プロファイル 5 個、ROM 3 個、ボタン 11。
const PROFILES: u8 = 5;
const ROMS: u8 = 3;

pub(super) fn frame(feat: u8, addr: u8, params: &[u8]) -> Vec<u8> {
    let mut v = vec![0x11, IDX, feat, addr];
    v.extend_from_slice(params);
    v
}

fn feature_reply(h: &Hidpp, index: u8) {
    h.ch.mock_reply(&frame(0, 0x0A, &[index, 0, 0]));
}

fn info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId("hid:046d:c08b:test".into()),
        kind: DeviceKind::Mouse,
        name: "Logitech G502 HERO".into(),
        vendor: "Logitech".into(),
        model: "g502-hero".into(),
        vid: 0x046D,
        pid: 0xC08B,
        transport: Transport::Wired,
        firmware: None,
        dongle_firmware: None,
        serial: None,
        driver: DriverKind::Hidpp,
        battery: None,
        supported: true,
        asleep: false,
        board_id: None,
    }
}

/// open の応答: desc → DPI リスト (100–25600 step 50) → レート (1/2/4/8ms)。
pub(super) fn open_mock() -> HidppDriver {
    let h = Hidpp::new(Channel::mock(), IDX);
    feature_reply(&h, F_ONB);
    let desc = [1, 2, 1, PROFILES, ROMS, 11, 16, 0x01, 0x00, 10, 1];
    h.ch.mock_reply(&frame(F_ONB, 0x0A, &desc));
    feature_reply(&h, F_DPI);
    h.ch.mock_reply(&frame(F_DPI, 0x1A, &[0, 0x00, 0x64, 0xE0, 0x32, 0x64, 0x00, 0, 0]));
    feature_reply(&h, F_RATE);
    h.ch.mock_reply(&frame(F_RATE, 0x0A, &[0b1000_1011]));
    open_with(h, info()).unwrap()
}

/// memoryRead の応答としてセクタ 1 個分を積む (read_sector と同じオフセット列)。
pub(super) fn queue_sector(d: &HidppDriver, bytes: &[u8]) {
    let size = bytes.len();
    let mut offset = 0;
    while offset < size {
        let at = if size - offset < 16 { size - 16 } else { offset };
        d.hp_for_test().ch.mock_reply(&frame(F_ONB, 0x5A, &bytes[at..at + 16]));
        offset += 16;
    }
}

/// CRC 正のプロファイルセクタ (rate 1000/ms、既定段、DPI 5 段)。
pub(super) fn profile_sector(rate_ms: u8, default_idx: u8, dpi: [u16; 5]) -> Vec<u8> {
    let mut b = vec![0xFF_u8; SECTOR];
    b[0] = rate_ms;
    b[1] = default_idx;
    b[2] = 0;
    for (i, v) in dpi.iter().enumerate() {
        b[3 + 2 * i..5 + 2 * i].copy_from_slice(&v.to_le_bytes());
    }
    b[13..32].fill(0);
    let crc = crc_ccitt(&b[..SECTOR - 2]);
    b[SECTOR - 2..].copy_from_slice(&crc.to_be_bytes());
    b
}

pub(super) fn directory_sector() -> Vec<u8> {
    let entries = (1..=u16::from(PROFILES))
        .map(|a| DirectoryEntry { address: a, enabled: true })
        .collect();
    serialize_directory(&ProfileDirectory { entries }, SECTOR)
}

/// memoryRead 要求の sector (params 先頭 BE16) の列。
pub(super) fn read_sectors(d: &HidppDriver) -> Vec<u16> {
    d.hp_for_test()
        .ch
        .mock_sent()
        .iter()
        .filter(|f| f[2] == F_ONB && f[3] == 0x5A)
        .map(|f| u16::from_be_bytes([f[4], f[5]]))
        .collect()
}

#[test]
fn open_builds_caps_from_device_replies() {
    let d = open_mock();
    let caps = d.caps();
    let m = caps.mouse.as_ref().unwrap();
    assert_eq!((m.dpi_min, m.dpi_max, m.dpi_step), (100, 25600, 50));
    assert_eq!((m.min_stages, m.max_stages), (1, 5));
    assert_eq!(m.buttons.len(), 11);
    assert_eq!(m.buttons[0].name, "G1 左クリック");
    assert_eq!(m.buttons[10].name, "ホイール左チルト");
    assert_eq!(m.button_kinds.len(), 6);
    assert!(m.require_left_click);
    assert!(caps.lighting.is_empty());
    let spec = caps.setting(SETTING_POLLING).unwrap();
    let SettingKind::Choice { options } = &spec.kind else {
        panic!("polling は choice");
    };
    let hz: Vec<i64> = options.iter().map(|o| o.value).collect();
    assert_eq!(hz, vec![125, 250, 500, 1000]);
}

#[test]
fn read_user_profile_gives_stages_and_polling() {
    let d = open_mock();
    let h = d.hp_for_test();
    // getCurrentProfile → params[1] = 2 (1 始まり) → index 1 = sector 0x0002
    h.ch.mock_reply(&frame(F_ONB, 0x4A, &[0, 2]));
    queue_sector(&d, &directory_sector());
    queue_sector(&d, &profile_sector(2, 1, [400, 800, 1600, 3200, 0]));
    // getSensorDpi → 現在 1600 (ステージ 2)
    h.ch.mock_reply(&frame(F_DPI, 0x2A, &[0, 0x06, 0x40, 0x03, 0x20]));
    let s = d.read().unwrap();
    assert_eq!(s.profile.active, 1);
    let m = s.mouse.unwrap();
    let dpi: Vec<u32> = m.stages.iter().map(|st| st.x).collect();
    assert_eq!(dpi, vec![400, 800, 1600, 3200]);
    assert_eq!(m.active_stage, 2);
    assert_eq!(s.settings[SETTING_POLLING], SettingValue::Choice(500));
    assert_eq!(read_sectors(&d)[0], 0x0000);
    assert_eq!(*read_sectors(&d).last().unwrap(), 0x0002);
}

#[test]
fn read_user_profile_exposes_buttons() {
    let d = open_mock();
    let h = d.hp_for_test();
    h.ch.mock_reply(&frame(F_ONB, 0x4A, &[0, 2]));
    queue_sector(&d, &directory_sector());
    let mut sector = profile_sector(2, 1, [400, 800, 1600, 3200, 0]);
    // probe3.log entry 0 と同じ割当 (G502 既定) を埋める。
    let bindings = [
        [0x80, 0x01, 0x00, 0x01], // G1 左
        [0x80, 0x01, 0x00, 0x02], // G2 右
        [0x80, 0x01, 0x00, 0x04], // G3 中
        [0x80, 0x01, 0x00, 0x08], // G4 戻る
        [0x80, 0x01, 0x00, 0x10], // G5 進む
        [0x90, 0x07, 0xff, 0xff], // G6 DpiShift (スナイパー)
        [0x90, 0x04, 0xff, 0xff], // G7 DpiDown
        [0x90, 0x03, 0xff, 0xff], // G8 DpiUp
        [0x90, 0x0a, 0xff, 0xff], // G9 ProfileCycle
        [0x90, 0x02, 0xff, 0xff], // ホイール右
        [0x90, 0x01, 0xff, 0xff], // ホイール左
    ];
    for (slot, b) in bindings.iter().enumerate() {
        sector[32 + 4 * slot..36 + 4 * slot].copy_from_slice(b);
    }
    let crc = crc_ccitt(&sector[..SECTOR - 2]);
    sector[SECTOR - 2..].copy_from_slice(&crc.to_be_bytes());
    queue_sector(&d, &sector);
    h.ch.mock_reply(&frame(F_DPI, 0x2A, &[0, 0x06, 0x40, 0x03, 0x20]));
    let s = d.read().unwrap();
    let m = s.mouse.unwrap();
    assert_eq!(m.buttons.len(), 11);
    assert_eq!(m.buttons[0], ButtonAction::Mouse(MouseFn::Left));
    assert_eq!(m.buttons[5], ButtonAction::Dpi(DpiFn::Shift(400)));
    assert_eq!(m.buttons[8], ButtonAction::ProfileCycle);
    // ホイールチルトは MouseFn::ScrollRight として出る。
    assert_eq!(m.buttons[9], ButtonAction::Mouse(MouseFn::ScrollRight));
    assert_eq!(m.buttons[10], ButtonAction::Mouse(MouseFn::ScrollLeft));
}

#[test]
fn read_falls_back_to_rom_when_directory_crc_is_broken() {
    let d = open_mock();
    let h = d.hp_for_test();
    h.ch.mock_reply(&frame(F_ONB, 0x4A, &[0, 3]));
    let mut dir = directory_sector();
    dir[SECTOR - 1] ^= 0xFF;
    queue_sector(&d, &dir);
    // ROM は CRC 検査なし (libratbag 同様) — CRC を壊しても読める。
    let mut rom = profile_sector(1, 0, [800, 1200, 0, 0, 0]);
    rom[SECTOR - 2] ^= 0xFF;
    queue_sector(&d, &rom);
    // 現在 DPI がどの段とも一致しない → 既定段 (0)。
    h.ch.mock_reply(&frame(F_DPI, 0x2A, &[0, 0x03, 0x84, 0x03, 0x20]));
    let s = d.read().unwrap();
    let m = s.mouse.unwrap();
    assert_eq!(m.stages.len(), 2);
    assert_eq!(m.active_stage, 0);
    assert_eq!(s.settings[SETTING_POLLING], SettingValue::Choice(1000));
    // raw 3 → index 2 (< ROM 数 3) → ROM 0x0103。
    assert_eq!(*read_sectors(&d).last().unwrap(), 0x0103);
}

#[test]
fn profile_index_is_one_based() {
    use super::driver_read::profile_index;
    assert_eq!(profile_index(1, PROFILES).unwrap(), 0);
    assert_eq!(profile_index(3, PROFILES).unwrap(), 2);
    assert_eq!(profile_index(PROFILES, PROFILES).unwrap(), PROFILES - 1);
    assert!(profile_index(0, PROFILES).is_err());
    assert!(profile_index(PROFILES + 1, PROFILES).is_err());
}

#[test]
fn read_rejects_current_profile_zero() {
    let d = open_mock();
    d.hp_for_test().ch.mock_reply(&frame(F_ONB, 0x4A, &[0, 0]));
    assert!(d.read().is_err());
    // 不正な index でセクタは読まない。
    assert!(read_sectors(&d).is_empty());
}

#[test]
fn rom_sector_uses_first_rom_beyond_rom_count() {
    use super::driver_read::rom_sector;
    assert_eq!(rom_sector(0, 3), 0x0101);
    assert_eq!(rom_sector(2, 3), 0x0103);
    assert_eq!(rom_sector(4, 3), 0x0101);
}
