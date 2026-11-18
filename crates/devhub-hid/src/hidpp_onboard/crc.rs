//! CRC-CCITT (HID++ 2.0 オンボードプロファイル用)。
//!
//! 初期値 0xFFFF・多項式 0x1021。プロファイルセクタ・ディレクトリの末尾 2 バイトに
//! big-endian で格納する。Logitech が libratbag に提供した実装
//! (libratbag src/hidpp-generic.c, MIT) と同じアルゴリズム。

/// CRC-CCITT (初期値 0xFFFF、多項式 0x1021) を計算する。
pub fn crc_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xffff;
    for &byte in data {
        let temp = (crc >> 8) ^ u16::from(byte);
        crc <<= 8;
        let mut quick = temp ^ (temp >> 4);
        crc ^= quick;
        quick <<= 5;
        crc ^= quick;
        quick <<= 7;
        crc ^= quick;
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::crc_ccitt;

    #[test]
    fn crc_ccitt_matches_known_check_value() {
        // CRC-16/CCITT-FALSE の既知チェック値。
        assert_eq!(crc_ccitt(b"123456789"), 0x29b1);
    }

    #[test]
    fn crc_ccitt_of_empty_input_returns_seed() {
        assert_eq!(crc_ccitt(&[]), 0xffff);
    }
}
