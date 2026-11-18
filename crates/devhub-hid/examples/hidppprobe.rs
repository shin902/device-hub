//! Logitech G502 HERO (046d:c08b) の HID++ 2.0 診断 — 既定は**読み取りのみ**。
//!
//! 開く → protocol_version / FW / 名前 / DPI リスト・現在値 / レポートレート /
//! onboard desc / mode / current_profile / ディレクトリと現在プロファイルの
//! セクタ 16 進ダンプ + parse 結果を表示する。`--write-test` を付けない限り
//! 書き込み系 (set_* / write_sector / set_onboard_mode) は一切呼ばない。macOS / Windows どちらでも動く。
//!
//! ```text
//! cargo run -p devhub-hid --example hidppprobe                 # 現在プロファイルだけ
//! cargo run -p devhub-hid --example hidppprobe -- --all        # ディレクトリの全エントリ
//! cargo run -p devhub-hid --example hidppprobe -- --write-test # 現在プロファイルに同じバイトを書き戻す
//! ```
//!
//! `--write-test` はドライバと同じ書き込み経路 (`driver_write::write_active_profile`) で
//! 現在プロファイルを**変更なし**で書き直し、読み戻して元のバイトと比べて OK / 差分を出す。
//! 再エンコードが読み出しと一致しない場合や ROM プロファイル・ホストモードでは書かずに中止する。
//!
//! `--all` はディレクトリの各エントリのセクタを memoryRead だけで読み、
//! 16 進ダンプと要約 (レート・既定/シフト段・DPI・ボタン・LED) を出す。

use devhub_hid::hidpp::driver_read::{profile_index, read_directory, read_profile};
use devhub_hid::hidpp::driver_write::{diff_offsets, write_active_profile};
use devhub_hid::hidpp::onboard_io::OnboardProfilesDesc;
use devhub_hid::hidpp::proto::{Hidpp, DEV_IDX_WIRED};
use devhub_hid::hidpp_onboard::{profile, OnboardProfile, ProfileDirectory};
use devhub_hid::transport::Channel;

const VID: u16 = 0x046D;
const PID: u16 = 0xC08B;
/// HID++ long collection (report 0x11)。
const USAGE_PAGE: u16 = 0xFF00;
const USAGE: u16 = 0x02;
const LONG_REPORT_LEN: usize = 20;

fn main() {
    let all = std::env::args().any(|a| a == "--all");
    let write_test = std::env::args().any(|a| a == "--write-test");
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

    if all {
        dump_all(&hp, desc.sector_size, desc.button_count, dir.as_ref());
        return;
    }

    // getCurrentProfile は 1 始まり → 0 始まりの index へ (driver と同じ変換)。
    let raw = hp.current_profile();
    let index = match &raw {
        Ok(Some(r)) => profile_index(*r, desc.profile_count),
        _ => Err(devhub_core::backend::HubError::Unsupported("getCurrentProfile")),
    };
    println!("\ncurrent_profile:  raw {raw:?} (getCurrentProfile params[1], 1 始まり) → index {index:?}");
    let index = match index {
        Ok(i) => i,
        Err(e) => {
            println!("現在プロファイルを決められない: {e}");
            return;
        }
    };
    let r = match read_profile(&hp, &desc, dir.as_ref(), index) {
        Ok(r) => r,
        Err(e) => {
            println!("profile index {index} read: {e}");
            return;
        }
    };
    println!(
        "\n== profile index {index}: sector {:#06x} ({:?}, {} B)",
        r.sector,
        r.source,
        r.bytes.len()
    );
    dump(&r.bytes);
    summary(&r.profile);
    if write_test {
        rewrite_same_bytes(&hp, &desc, r.sector, &r.bytes);
    }
}

/// `--write-test`: 変更なしで書き込み経路を通し、読み戻して元のバイトと比べる。
fn rewrite_same_bytes(hp: &Hidpp, desc: &OnboardProfilesDesc, sector: u16, original: &[u8]) {
    println!("\n== write-test: sector {sector:#06x} に同じバイトを書き戻す");
    if let Err(e) = write_active_profile(hp, desc, |_| Ok(())) {
        println!("write-test: 中止/失敗: {e}");
        return;
    }
    match hp.read_sector(sector, desc.sector_size) {
        Ok(back) => {
            let diff = diff_offsets(original, &back);
            if diff.is_empty() {
                println!("write-test: OK (読み戻し {} B が元と一致)", back.len());
            } else {
                println!("write-test: 差分 {} バイト offset {diff:?}", diff.len());
                dump(&back);
            }
        }
        Err(e) => println!("write-test: 読み戻し失敗: {e}"),
    }
}

/// `--all`: ディレクトリの全エントリのセクタをダンプ + 要約 (memoryRead のみ)。
fn dump_all(hp: &Hidpp, size: u16, button_count: u8, dir: Option<&ProfileDirectory>) {
    let Some(dir) = dir else {
        println!("\n--all: ディレクトリの CRC が不一致なのでエントリが無い");
        return;
    };
    let buttons = usize::from(button_count).min(profile::MAX_BUTTONS);
    for (i, entry) in dir.entries.iter().enumerate() {
        println!(
            "\n== entry {i}: sector {:#06x} (enabled: {})",
            entry.address, entry.enabled
        );
        match hp.read_sector(entry.address, size) {
            Ok(bytes) => {
                dump(&bytes);
                match profile::parse(&bytes, usize::from(size), buttons) {
                    Ok(p) => summary(&p),
                    Err(e) => println!("parse: {e}"),
                }
            }
            Err(e) => println!("read: {e}"),
        }
    }
}

/// プロファイルの要約。
fn summary(p: &OnboardProfile) {
    println!("report_rate_hz:    {}", p.report_rate_hz);
    println!("default_dpi_index: {}", p.default_dpi_index);
    println!("shift_dpi_index:   {}", p.shift_dpi_index);
    println!("dpi:               {:?}", p.dpi);
    println!("name:              {:?}", p.name);
    println!("buttons:           {:?}", p.buttons);
    println!("leds:              {:?}", p.leds);
}

/// 16 バイトごとの 16 進ダンプ。
fn dump(bytes: &[u8]) {
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        println!("  {:04x}: {}", row * 16, hex.join(" "));
    }
}
