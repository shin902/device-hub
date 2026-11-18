//! `features` の単体テスト (250 行ルールのため `features.rs` から分離)。

use super::features::*;
use super::proto::Hidpp;
use crate::transport::Channel;

const IDX: u8 = 0xFF;

fn frame(feat: u8, addr: u8, params: &[u8]) -> Vec<u8> {
    let mut v = vec![0x11, IDX, feat, addr];
    v.extend_from_slice(params);
    v
}

fn mock() -> Hidpp {
    Hidpp::new(Channel::mock(), IDX)
}

#[test]
fn dpi_list_range_encoding() {
    let r = [0u8, 0x00, 0x64, 0xE0, 0x32, 0x3E, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        parse_dpi_list(&r),
        DpiList::Range { min: 100, max: 16000, step: 50 }
    );
}

#[test]
fn dpi_list_values_encoding() {
    let r = [0x00u8, 0x03, 0xE8, 0x07, 0xD0, 0x0B, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(parse_dpi_list(&r), DpiList::Values(vec![1000, 2000, 3000]));
}

#[test]
fn report_rate_bitmap_maps_bits_to_ms() {
    // bit n -> (n+1) ms: 0x0F (bit 0-3) -> 1,2,3,4 ms。
    // TASK.md の期待例 [1,2,4,8] は bit 0,1,3,7 が立つビットマスク 0x8B。
    assert_eq!(parse_rate_bitmap(0x0F), vec![1, 2, 3, 4]);
    assert_eq!(parse_rate_bitmap(0x8B), vec![1, 2, 4, 8]);
}

#[test]
fn fw_entities_and_main_firmware_string() {
    let h = mock();
    // getFeature 0x0003 -> index 3
    h.ch.mock_reply(&frame(0, 0x0A, &[3, 0, 0, 0]));
    // count = 2
    h.ch.mock_reply(&frame(3, 0x0A, &[2, 0, 0, 0]));
    // entity 0: kind 0, name "U1 ", major 0x12, minor 3, build 45
    h.ch.mock_reply(&frame(3, 0x1A, &[0x00, b'U', b'1', b' ', 0x12, 3, 0, 45]));
    // entity 1: kind 1 (BL), name "BL "
    h.ch.mock_reply(&frame(3, 0x1A, &[0x01, b'B', b'L', b' ', 0x01, 2, 0, 99]));
    let entities = h.fw_entities().unwrap().unwrap();
    assert_eq!(entities.len(), 2);
    assert_eq!(entities[0].name, "U1 ");
    // main_firmware_string は fw_entities を再び呼ぶので応答を積み直す
    // (getFeature はキャッシュ済みで再送信されない)。
    h.ch.mock_reply(&frame(3, 0x0A, &[2, 0, 0, 0]));
    h.ch.mock_reply(&frame(3, 0x1A, &[0x00, b'U', b'1', b' ', 0x12, 3, 0, 45]));
    h.ch.mock_reply(&frame(3, 0x1A, &[0x01, b'B', b'L', b' ', 0x01, 2, 0, 99]));
    let fw = h.main_firmware_string().unwrap().unwrap();
    // major 0x12, minor 0x03 (BCD) -> "12.03", build 45 -> "B002D"
    assert_eq!(fw, "U1 12.03.B002D");
}

#[test]
fn device_name_reads_in_chunks() {
    let h = mock();
    // getFeature 0x0005 -> index 2
    h.ch.mock_reply(&frame(0, 0x0A, &[2, 0, 0, 0]));
    // name length = 20 ("G502 HERO Wireless" = 18... use 20 bytes)
    h.ch.mock_reply(&frame(2, 0x0A, &[20, 0, 0, 0]));
    // chunk 0: 16 bytes
    h.ch.mock_reply(&frame(2, 0x1A, b"G502 HERO Wirele"));
    // chunk 16: remaining 4 bytes
    h.ch.mock_reply(&frame(2, 0x1A, b"ss!!"));
    let name = h.device_name().unwrap().unwrap();
    assert_eq!(name, "G502 HERO Wireless!!");
}
