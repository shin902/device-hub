//! STEP 3: FW 文字列・プロファイルセクタのキャッシュと、変化なし書き込みの省略。
//! 要求数はモックの送信フレーム数で数える。

use super::driver::HidppDriver;
use super::tests_driver::{directory_sector, frame, open_mock, queue_sector, F_DPI, F_ONB};
use super::tests_g502_sectors::{entry0, entry1};
use devhub_core::backend::DeviceDriver;
use devhub_core::model::*;

fn sent(d: &HidppDriver) -> Vec<Vec<u8>> {
    d.hp_for_test().ch.mock_sent()
}

/// getCurrentProfile (raw = 1 始まり) と getSensorDpi (1200) の応答。
fn queue_current(d: &HidppDriver, raw: u8) {
    d.hp_for_test().ch.mock_reply(&frame(F_ONB, 0x4A, &[0, raw]));
}

fn queue_sensor_dpi(d: &HidppDriver) {
    d.hp_for_test().ch.mock_reply(&frame(F_DPI, 0x2A, &[0, 0x04, 0xB0, 0x04, 0xB0]));
}

/// 1 回目の read (ディレクトリ + プロファイルを読む)。
fn first_read(d: &HidppDriver, raw: u8, sector: &[u8]) -> DeviceState {
    queue_current(d, raw);
    queue_sector(d, &directory_sector());
    queue_sector(d, sector);
    queue_sensor_dpi(d);
    d.read().unwrap()
}

/// 送信フレームの (feature index, func<<4|sw_id)。
fn kinds(frames: &[Vec<u8>]) -> Vec<(u8, u8)> {
    frames.iter().map(|f| (f[2], f[3])).collect()
}

#[test]
fn info_uses_firmware_cached_at_open() {
    let d = open_mock();
    let before = sent(&d).len();
    let a = d.info();
    let b = d.info();
    assert_eq!(a.firmware.as_deref(), Some("U1 12.03.B0045"));
    assert_eq!(b.firmware, a.firmware);
    assert!(a.supported);
    assert_eq!(sent(&d).len(), before);
}

#[test]
fn second_read_only_asks_current_profile_and_sensor_dpi() {
    let d = open_mock();
    let s1 = first_read(&d, 1, &entry0());
    let before = sent(&d).len();
    queue_current(&d, 1);
    queue_sensor_dpi(&d);
    let s2 = d.read().unwrap();
    assert_eq!(kinds(&sent(&d)[before..]), vec![(F_ONB, 0x4A), (F_DPI, 0x2A)]);
    assert_eq!(s1, s2);
    assert_eq!(d.request_count() as usize, sent(&d).len());
}

#[test]
fn profile_index_change_rereads_directory_and_profile() {
    let d = open_mock();
    first_read(&d, 1, &entry0());
    let before = sent(&d).len();
    let s = first_read(&d, 2, &entry1());
    let reads: Vec<u16> = sent(&d)[before..]
        .iter()
        .filter(|f| f[2] == F_ONB && f[3] == 0x5A)
        .map(|f| u16::from_be_bytes([f[4], f[5]]))
        .collect();
    // ディレクトリ 16 回 + sector 0x0002 を 16 回。
    assert_eq!(reads.len(), 32);
    assert_eq!(reads[0], 0x0000);
    assert_eq!(*reads.last().unwrap(), 0x0002);
    assert_eq!(s.profile.active, 1);
    let dpi: Vec<u32> = s.mouse.unwrap().stages.iter().map(|st| st.x).collect();
    assert_eq!(dpi, vec![400, 800, 1600, 2400]);
}

#[test]
fn no_op_edit_sends_no_write() {
    let d = open_mock();
    first_read(&d, 1, &entry0());
    let before = sent(&d).len();
    let ch = &d.hp_for_test().ch;
    ch.mock_reply(&frame(F_ONB, 0x2A, &[1]));
    queue_current(&d, 1);
    // G1 は既に左クリック → 同じ割当にしてもバイトは変わらない。
    let same = Change::Button { slot: 0, action: ButtonAction::Mouse(MouseFn::Left) };
    d.apply(&same).unwrap();
    // getOnboardMode + getCurrentProfile だけ (セクタはキャッシュ、書き込みと読み戻しは省略)。
    assert_eq!(kinds(&sent(&d)[before..]), vec![(F_ONB, 0x2A), (F_ONB, 0x4A)]);
}

#[test]
fn write_updates_cache_with_verified_bytes() {
    let d = open_mock();
    first_read(&d, 1, &entry0());
    let ch = &d.hp_for_test().ch;
    // G2 を無効化: ボタン 1 の 4 バイトが ff ff ff ff。
    let mut want = entry0();
    want[36..40].copy_from_slice(&[0xff; 4]);
    let crc = crate::hidpp_onboard::crc::crc_ccitt(&want[..254]);
    want[254..].copy_from_slice(&crc.to_be_bytes());
    ch.mock_reply(&frame(F_ONB, 0x2A, &[1]));
    queue_current(&d, 1);
    ch.mock_reply(&frame(F_ONB, 0x6A, &[]));
    for _ in 0..16 {
        ch.mock_reply(&frame(F_ONB, 0x7A, &[]));
    }
    ch.mock_reply(&frame(F_ONB, 0x8A, &[]));
    queue_sector(&d, &want);
    d.apply(&Change::Button { slot: 1, action: ButtonAction::Disabled }).unwrap();
    // 書いた後の read はセクタを読まずに新しい割当を返す。
    let before = sent(&d).len();
    queue_current(&d, 1);
    queue_sensor_dpi(&d);
    let s = d.read().unwrap();
    assert_eq!(kinds(&sent(&d)[before..]), vec![(F_ONB, 0x4A), (F_DPI, 0x2A)]);
    assert_eq!(s.mouse.unwrap().buttons[1], ButtonAction::Disabled);
}

#[test]
fn failed_write_invalidates_cache() {
    let d = open_mock();
    first_read(&d, 1, &entry0());
    let ch = &d.hp_for_test().ch;
    ch.mock_reply(&frame(F_ONB, 0x2A, &[1]));
    queue_current(&d, 1);
    ch.mock_reply(&frame(F_ONB, 0x6A, &[]));
    for _ in 0..16 {
        ch.mock_reply(&frame(F_ONB, 0x7A, &[]));
    }
    ch.mock_reply(&frame(F_ONB, 0x8A, &[]));
    // 読み戻しが元のまま → Protocol エラー。
    queue_sector(&d, &entry0());
    assert!(d.apply(&Change::Button { slot: 1, action: ButtonAction::Disabled }).is_err());
    // 次の read はセクタを読み直す。
    let before = sent(&d).len();
    first_read(&d, 1, &entry0());
    let memory_reads = sent(&d)[before..].iter().filter(|f| f[3] == 0x5A).count();
    assert_eq!(memory_reads, 32);
}
