//! `onboard_io` の単体テスト (250 行ルールのため `onboard_io.rs` から分離)。

use super::proto::Hidpp;
use crate::transport::Channel;

const IDX: u8 = 0xFF;
/// OnboardProfiles の feature index (getFeature の応答で返す値)。
const FEAT: u8 = 0x02;

fn frame(feat: u8, addr: u8, params: &[u8]) -> Vec<u8> {
    let mut v = vec![0x11, IDX, feat, addr];
    v.extend_from_slice(params);
    v
}

fn mock() -> Hidpp {
    Hidpp::new(Channel::mock(), IDX)
}

/// getFeature 0x8100 → FEAT の応答を積む。
fn mock_feature(h: &Hidpp) {
    h.ch.mock_reply(&frame(0, 0x0A, &[FEAT, 0, 0, 0]));
}

/// getProfilesDescription の応答 (G502 HERO 風)。
fn desc_reply() -> [u8; 16] {
    [
        1,  // memory_model_id (G402 type)
        2,  // profile_format_id
        2,  // macro_format_id
        3,  // profile_count
        1,  // profile_count_oob
        11, // button_count
        5,  // sector_count
        0x00, 0x80, // sector_size = 128 (BE)
        1,  // mechanical_layout
        0,  // various_info
        0, 0, 0, 0, 0,
    ]
}

#[test]
fn profiles_desc_parses_fields() {
    let h = mock();
    mock_feature(&h);
    h.ch.mock_reply(&frame(FEAT, 0x0A, &desc_reply()));
    let d = h.onboard_profiles_desc().unwrap().unwrap();
    assert_eq!(d.memory_model_id, 1);
    assert_eq!(d.profile_count, 3);
    assert_eq!(d.button_count, 11);
    assert_eq!(d.sector_count, 5);
    assert_eq!(d.sector_size, 128);
    assert_eq!(d.mechanical_layout, 1);
}

#[test]
fn read_sector_requests_offsets_and_pads_last_chunk() {
    let h = mock();
    mock_feature(&h);
    // sector_size 40: offset 0, 16, then last chunk reads at 40-16=24.
    for offset in [0u16, 16, 24] {
        let mut params = [0u8; 16];
        params[0..2].copy_from_slice(&3u16.to_be_bytes()); // sector 3
        params[2..4].copy_from_slice(&offset.to_be_bytes());
        h.ch.mock_reply(&frame(FEAT, 0x5A, &params));
    }
    let data = h.read_sector(3, 40).unwrap();
    assert_eq!(data.len(), 40);
    let sent = h.ch.mock_sent();
    assert_eq!(sent.len(), 4); // getFeature + 3 reads
    assert_eq!(sent[1][3], 0x5A); // memoryRead
    assert_eq!(&sent[1][4..6], &3u16.to_be_bytes());
    assert_eq!(&sent[3][6..8], &24u16.to_be_bytes()); // padded last chunk
}

#[test]
fn write_sector_requests_addr_write_chunks_and_end() {
    let h = mock();
    mock_feature(&h);
    h.ch.mock_reply(&frame(FEAT, 0x6A, &[0; 16])); // AddrWrite
    for _ in 0..3 {
        h.ch.mock_reply(&frame(FEAT, 0x7A, &[0; 16])); // Write
    }
    h.ch.mock_reply(&frame(FEAT, 0x8A, &[0; 16])); // WriteEnd
    let data = vec![0xA5u8; 48];
    h.write_sector(3, &data).unwrap();
    let sent = h.ch.mock_sent();
    assert_eq!(sent.len(), 6); // getFeature + AddrWrite + 3x Write + WriteEnd
    assert_eq!(sent[1][3], 0x6A); // AddrWrite
    assert_eq!(&sent[1][4..6], &3u16.to_be_bytes());
    assert_eq!(&sent[1][6..8], &0u16.to_be_bytes());
    assert_eq!(&sent[1][8..10], &48u16.to_be_bytes());
    assert_eq!(sent[2][3], 0x7A); // Write chunk
    assert_eq!(sent[5][3], 0x8A); // WriteEnd
}

#[test]
fn write_sector_pads_partial_chunk_for_255_byte_sector() {
    let h = mock();
    mock_feature(&h);
    h.ch.mock_reply(&frame(FEAT, 0x6A, &[0; 16])); // AddrWrite
    for _ in 0..16 {
        h.ch.mock_reply(&frame(FEAT, 0x7A, &[0; 16])); // Write
    }
    h.ch.mock_reply(&frame(FEAT, 0x8A, &[0; 16])); // WriteEnd
    let data = vec![0x5Au8; 255];
    h.write_sector(3, &data).unwrap();
    let sent = h.ch.mock_sent();
    // getFeature + AddrWrite + 16x Write (255 = 15x16 + 15) + WriteEnd
    assert_eq!(sent.len(), 19);
    assert_eq!(&sent[1][8..10], &255u16.to_be_bytes()); // count = sector_size
    assert_eq!(sent[2][3], 0x7A); // Write chunk
    // Last write: 15 bytes of data + 1 zero pad.
    let last = &sent[17];
    assert_eq!(&last[4..19], &data[240..255]);
    assert_eq!(last[19], 0);
    assert_eq!(sent[18][3], 0x8A); // WriteEnd
}
