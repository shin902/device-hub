//! Logitech G502 HERO (046d:c08b) の HID++ 2.0 診断 — **読み取りのみ**。
//!
//! 開く → protocol_version / FW / 名前 / DPI リスト・現在値 / レポートレート /
//! onboard desc / mode / current_profile / ディレクトリと現在プロファイルの
//! セクタ 16 進ダンプ + parse 結果を表示する。書き込み系 (set_* / write_sector /
//! set_onboard_mode) は一切呼ばない。macOS / Windows どちらでも動く。
//!
//! ```text
//! cargo run -p devhub-hid --example hidppprobe
//! ```

use devhub_hid::hidpp::driver_read::{read_directory, read_profile};
use devhub_hid::hidpp::proto::{Hidpp, DEV_IDX_WIRED};
use devhub_hid::transport::Channel;

const VID: u16 = 0x046D;
const PID: u16 = 0xC08B;
/// HID++ long collection (report 0x11)。
const USAGE_PAGE: u16 = 0xFF00;
const USAGE: u16 = 0x02;
const LONG_REPORT_LEN: usize = 20;

fn main() {
    let api = hidapi::HidApi::new().expect("hidapi init");
    let Some(info) = api.device_list().find(|d| {
        d.vendor_id() == VID
            && d.product_id() == PID
            && d.usage_page() == USAGE_PAGE
            && d.usage() == USAGE
    }) else {
        eprintln!("G502 HERO ({VID:04x}:{PID:04x} {USAGE_PAGE:04x}:{USAGE:02x}) が見つかりません");
        std::process::exit(1);
    };
    println!("path: {:?}", info.path());
    let dev = info.open_device(&api).expect("open");
    let hp = Hidpp::new(
        Channel::open(dev, LONG_REPORT_LEN, LONG_REPORT_LEN),
        DEV_IDX_WIRED,
    );

    println!("protocol_version: {:?}", hp.protocol_version());
    println!("firmware:         {:?}", hp.main_firmware_string());
    println!("fw entities:      {:?}", hp.fw_entities());
    println!("device_name:      {:?}", hp.device_name());
    println!("sensor_count:     {:?}", hp.sensor_count());
    println!("dpi_list(0):      {:?}", hp.sensor_dpi_list(0));
    println!("dpi(0):           {:?}  (current, default)", hp.sensor_dpi(0));
    println!("report_rate_list: {:?}  (ms)", hp.report_rate_list());
    println!("report_rate:      {:?}  (ms)", hp.report_rate());
    println!("onboard_mode:     {:?}  (1=onboard, 2=host)", hp.onboard_mode());
    println!("current_profile:  {:?}  (getCurrentProfile params[1])", hp.current_profile());

    let desc = match hp.onboard_profiles_desc() {
        Ok(Some(d)) => d,
        other => {
            println!("onboard desc:     {other:?}");
            return;
        }
    };
    println!("onboard desc:     {desc:?}");

    let dir = match read_directory(&hp, &desc) {
        Ok((bytes, dir)) => {
            println!("\n== directory sector 0x0000 ({} B)", bytes.len());
            dump(&bytes);
            println!("parse: {dir:?}  (None = CRC 不一致 → ROM)");
            dir
        }
        Err(e) => {
            println!("directory read: {e}");
            None
        }
    };

    let index = hp
        .current_profile()
        .ok()
        .flatten()
        .unwrap_or(0)
        .min(desc.profile_count.saturating_sub(1));
    match read_profile(&hp, &desc, dir.as_ref(), index) {
        Ok(r) => {
            println!(
                "\n== profile index {index}: sector {:#06x} ({:?}, {} B)",
                r.sector,
                r.source,
                r.bytes.len()
            );
            dump(&r.bytes);
            let p = &r.profile;
            println!("report_rate_hz:    {}", p.report_rate_hz);
            println!("default_dpi_index: {}", p.default_dpi_index);
            println!("shift_dpi_index:   {}", p.shift_dpi_index);
            println!("dpi:               {:?}", p.dpi);
            println!("name:              {:?}", p.name);
            println!("buttons:           {:?}", p.buttons);
            println!("leds:              {:?}", p.leds);
        }
        Err(e) => println!("profile index {index} read: {e}"),
    }
}

/// 16 バイトごとの 16 進ダンプ。
fn dump(bytes: &[u8]) {
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        println!("  {:04x}: {}", row * 16, hex.join(" "));
    }
}
