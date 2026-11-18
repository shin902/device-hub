//! `apply` の書き込み経路のモック Channel テスト (実機セクタ probe3 entry 0 を使う)。
//! 要求順: getOnboardMode → getCurrentProfile → ディレクトリ → プロファイル →
//! memoryAddrWrite → memoryWrite x16 → memoryWriteEnd → 読み戻し。

use super::driver::HidppDriver;
use super::driver_read::SETTING_POLLING;
use super::driver_write::diff_offsets;
use super::tests_driver::{directory_sector, frame, open_mock, queue_sector, F_DPI, F_ONB, SECTOR};
use super::tests_g502_sectors::{entry0, entry1};
use crate::hidpp_onboard::crc::crc_ccitt;
use crate::hidpp_onboard::profile;
use devhub_core::backend::{DeviceDriver, HubError};
use devhub_core::model::*;

/// getOnboardMode・getCurrentProfile (raw 1 = index 0 = sector 0x0001)・ディレクトリ・
/// プロファイルの応答を積む。
fn queue_reads(d: &HidppDriver, mode: u8, dir: &[u8], sector: &[u8]) {
    let ch = &d.hp_for_test().ch;
    ch.mock_reply(&frame(F_ONB, 0x2A, &[mode]));
    ch.mock_reply(&frame(F_ONB, 0x4A, &[0, 1]));
    queue_sector(d, dir);
    queue_sector(d, sector);
}

/// 書き込みの ACK (addrWrite + write x16 + writeEnd) と読み戻しの応答を積む。
fn queue_write(d: &HidppDriver, read_back: &[u8]) {
    let ch = &d.hp_for_test().ch;
    ch.mock_reply(&frame(F_ONB, 0x6A, &[]));
    for _ in 0..SECTOR / 16 {
        ch.mock_reply(&frame(F_ONB, 0x7A, &[]));
    }
    ch.mock_reply(&frame(F_ONB, 0x8A, &[]));
    queue_sector(d, read_back);
}

/// memoryWrite で送られたバイト列 (送信順に連結)。
fn written(d: &HidppDriver) -> Vec<u8> {
    let sent = d.hp_for_test().ch.mock_sent();
    let data: Vec<u8> = sent
        .iter()
        .filter(|f| f[2] == F_ONB && f[3] == 0x7A)
        .flat_map(|f| f[4..20].to_vec())
        .collect();
    data
}

fn wrote_anything(d: &HidppDriver) -> bool {
    d.hp_for_test().ch.mock_sent().iter().any(|f| f[2] == F_ONB && f[3] == 0x6A)
}

/// `orig` を `edit` で書き換えて CRC を付け直した、書き込み期待値。
fn expected(orig: &[u8], edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut b = orig.to_vec();
    edit(&mut b);
    let crc = crc_ccitt(&b[..SECTOR - 2]);
    b[SECTOR - 2..].copy_from_slice(&crc.to_be_bytes());
    b
}

/// 書いたバイトが期待値と一致し、差分が `allowed` + CRC に収まることを確認する。
fn assert_written(d: &HidppDriver, want: &[u8], allowed: std::ops::Range<usize>) {
    let got = written(d);
    assert_eq!(got, want);
    for off in diff_offsets(&entry0(), &got) {
        assert!(allowed.contains(&off) || off >= SECTOR - 2, "想定外の差分 offset {off}");
    }
}

#[test]
fn real_sectors_roundtrip_through_parse_and_serialize() {
    for bytes in [entry0(), entry1()] {
        let p = profile::parse(&bytes, SECTOR, 11).unwrap();
        assert_eq!(diff_offsets(&profile::serialize(&p, SECTOR), &bytes), Vec::<usize>::new());
    }
}

#[test]
fn button_write_key_with_mods_changes_only_that_slot() {
    let d = open_mock();
    let want = expected(&entry0(), |b| b[52..56].copy_from_slice(&[0x80, 0x02, 0x03, 0x04]));
    queue_reads(&d, 1, &directory_sector(), &entry0());
    queue_write(&d, &want);
    let key = KeyStroke { mods: mods::LCTRL | mods::LSHIFT, usage: 0x04 };
    d.apply(&Change::Button { slot: 5, action: ButtonAction::Key(key) }).unwrap();
    assert_written(&d, &want, 52..56);
    // 書き先はプロファイルと同じ sector 0x0001 (addrWrite params = [sector BE16, 0, len BE16])。
    let sent = d.hp_for_test().ch.mock_sent();
    let addr = sent.iter().find(|f| f[2] == F_ONB && f[3] == 0x6A).unwrap();
    assert_eq!(&addr[4..10], &[0x00, 0x01, 0x00, 0x00, 0x01, 0x00]);
}

#[test]
fn dpi_stages_write_zeroes_unused_slots_and_sets_default() {
    let d = open_mock();
    let want = expected(&entry0(), |b| {
        b[1] = 1;
        b[3..13].copy_from_slice(&[0x20, 0x03, 0x40, 0x06, 0, 0, 0, 0, 0, 0]);
    });
    queue_reads(&d, 1, &directory_sector(), &entry0());
    queue_write(&d, &want);
    let stage = |x| DpiStage { x, y: None, color: Rgb::new(0, 0, 0), enabled: true };
    d.apply(&Change::DpiStages { stages: vec![stage(800), stage(1600)], active: 1 }).unwrap();
    assert_written(&d, &want, 1..13);
}

#[test]
fn dpi_stages_reject_off_step_value_before_writing() {
    let d = open_mock();
    queue_reads(&d, 1, &directory_sector(), &entry0());
    let stage = DpiStage { x: 825, y: None, color: Rgb::new(0, 0, 0), enabled: true };
    let err = d.apply(&Change::DpiStages { stages: vec![stage], active: 0 }).unwrap_err();
    assert!(matches!(err, HubError::Invalid(_)));
    assert!(!wrote_anything(&d));
}

#[test]
fn polling_write_sets_report_interval() {
    let d = open_mock();
    let want = expected(&entry0(), |b| b[0] = 2);
    queue_reads(&d, 1, &directory_sector(), &entry0());
    queue_write(&d, &want);
    let change = Change::Setting { id: SETTING_POLLING.into(), value: SettingValue::Choice(500) };
    d.apply(&change).unwrap();
    assert_written(&d, &want, 0..1);
}

#[test]
fn rom_profile_is_refused() {
    let d = open_mock();
    let mut dir = directory_sector();
    dir[SECTOR - 1] ^= 0xFF;
    queue_reads(&d, 1, &dir, &entry0());
    let err = d
        .apply(&Change::Button { slot: 1, action: ButtonAction::Disabled })
        .unwrap_err();
    assert!(matches!(err, HubError::Unsupported("ROM プロファイル")), "{err:?}");
    assert!(!wrote_anything(&d));
}

#[test]
fn host_mode_is_refused_without_reading_profiles() {
    let d = open_mock();
    let before = d.hp_for_test().ch.mock_sent().len();
    d.hp_for_test().ch.mock_reply(&frame(F_ONB, 0x2A, &[2]));
    let err = d
        .apply(&Change::Button { slot: 1, action: ButtonAction::Disabled })
        .unwrap_err();
    assert!(matches!(err, HubError::Unsupported(_)));
    // getOnboardMode の 1 往復だけ。
    assert_eq!(d.hp_for_test().ch.mock_sent().len(), before + 1);
}

#[test]
fn left_click_guard_and_slot_range() {
    let d = open_mock();
    queue_reads(&d, 1, &directory_sector(), &entry0());
    let to_right = Change::Button { slot: 0, action: ButtonAction::Mouse(MouseFn::Right) };
    assert!(matches!(d.apply(&to_right).unwrap_err(), HubError::Invalid(_)));
    queue_reads(&d, 1, &directory_sector(), &entry0());
    let out_of_range = Change::Button { slot: 11, action: ButtonAction::Disabled };
    assert!(matches!(d.apply(&out_of_range).unwrap_err(), HubError::Invalid(_)));
    assert!(!wrote_anything(&d));
}

#[test]
fn read_back_mismatch_is_protocol_error() {
    let d = open_mock();
    queue_reads(&d, 1, &directory_sector(), &entry0());
    // 書き込みは成功したが、読み戻すと元のまま。
    queue_write(&d, &entry0());
    let err = d
        .apply(&Change::Button { slot: 1, action: ButtonAction::Disabled })
        .unwrap_err();
    assert!(matches!(err, HubError::Protocol(_)), "{err:?}");
}

#[test]
fn active_stage_sets_sensor_dpi_without_writing_profile() {
    let d = open_mock();
    let ch = &d.hp_for_test().ch;
    let queue_profile = || {
        ch.mock_reply(&frame(F_ONB, 0x4A, &[0, 1]));
        queue_sector(&d, &directory_sector());
        queue_sector(&d, &entry0());
    };
    queue_profile();
    ch.mock_reply(&frame(F_DPI, 0x3A, &[0, 0x0C, 0x80]));
    d.apply(&Change::ActiveStage(2)).unwrap();
    let sent = ch.mock_sent();
    let set = sent.last().unwrap();
    assert_eq!(&set[2..7], &[F_DPI, 0x3A, 0x00, 0x0C, 0x80]);
    assert!(!wrote_anything(&d));
    // entry 0 の有効段は 4 個 (0..=3)。
    queue_profile();
    assert!(matches!(d.apply(&Change::ActiveStage(4)), Err(HubError::Invalid(_))));
}

#[test]
fn other_changes_are_not_implemented() {
    let d = open_mock();
    let before = d.hp_for_test().ch.mock_sent().len();
    let err = d.apply(&Change::Profile(0)).unwrap_err();
    assert!(matches!(err, HubError::NotImplemented(_)));
    let lod = Change::Setting { id: "lod".into(), value: SettingValue::Choice(1) };
    assert!(matches!(d.apply(&lod).unwrap_err(), HubError::NotImplemented(_)));
    assert_eq!(d.hp_for_test().ch.mock_sent().len(), before);
}
