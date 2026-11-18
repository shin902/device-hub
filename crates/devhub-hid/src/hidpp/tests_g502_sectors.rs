//! G502 HERO 実機のプロファイルセクタ (`.build-logs/probe3.log` の entry 0 / 1、256B)。
//! entry 0 = LED Cycle (明るさ 0 = 100%)、entry 1 = LED Breathing。

const ENTRY0: [&str; 8] = [
    "010100b0046009800c00190000ffffffff00ffffffffffffffffffffffffffff",
    "80010001800100028001000480010008800100109007ffff9004ffff9003ffff",
    "900affff9002ffff9001ffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffff0300000000001f400000000300000000",
    "001f40000000ffffffffffffffffffffffffffffffffffffffffffffffff329a",
];

const ENTRY1: [&str; 8] = [
    "01020090012003400660090000ffffffff00ffffffffffffffffffffffffffff",
    "80010001800100028001000480010008800100109007ffff9004ffff9003ffff",
    "900affff9002ffff9001ffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffff0a00dcff1f4000000000000a00dcff1f",
    "400000000000ffffffffffffffffffffffffffffffffffffffffffffffff909c",
];

fn decode(rows: &[&str]) -> Vec<u8> {
    let hex: String = rows.concat();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// entry 0 (sector 0x0001): 1000Hz、既定段 1、DPI 1200/2400/3200/6400。
pub(super) fn entry0() -> Vec<u8> {
    decode(&ENTRY0)
}

/// entry 1 (sector 0x0002): 1000Hz、既定段 2、DPI 400/800/1600/2400。
pub(super) fn entry1() -> Vec<u8> {
    decode(&ENTRY1)
}
