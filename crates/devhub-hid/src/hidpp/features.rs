//! HID++ 2.0 機能ラッパ: 0x0003 DeviceFwVersion / 0x0005 DeviceName /
//! 0x2201 AdjustableDPI / 0x8060 ReportRate。
//! feature index は `Hidpp::feature()` で引き、無い機能は読み取り `Ok(None)`、
//! 書き込みは `HubError::Unsupported`。
//! (0x8100 OnboardProfiles は別ステップで追加する)。

use super::proto::Hidpp;
use devhub_core::backend::{HubError, HubResult};

/// DeviceFwVersion (hidpp20.h: `HIDPP_PAGE_DEVICE_INFO`)。
const FEAT_DEVICE_FW_VERSION: u16 = 0x0003;
/// DeviceName (hidpp20.h: `HIDPP_PAGE_DEVICE_NAME`)。
const FEAT_DEVICE_NAME: u16 = 0x0005;
/// AdjustableDPI (hidpp20.h: `HIDPP_PAGE_ADJUSTABLE_DPI`)。
const FEAT_ADJUSTABLE_DPI: u16 = 0x2201;
/// AdjustableReportRate (hidpp20.h: `HIDPP_PAGE_ADJUSTABLE_REPORT_RATE`)。
const FEAT_REPORT_RATE: u16 = 0x8060;

/// ファームウェアエンティティ 1 件。
#[derive(Debug, PartialEq)]
pub struct FwEntity {
    /// 種別の下位 4bit (0/1=ファームウェア層, 3=ハードウェア。hidpp20.py: `FirmwareKind`)。
    pub kind: u8,
    /// 3 文字の名前 (例 "U1 ")。
    pub name: String,
    pub major: u8,
    pub minor: u8,
    /// ビルド番号 (BE16)。
    pub build: u16,
}

/// getSensorDpiList のの解釈結果。
#[derive(Debug, PartialEq)]
pub enum DpiList {
    /// 値の列挙 (デバイスが返す順)。
    Values(Vec<u16>),
    /// `0xE0xx` 刻み指定によるレンジ (hidpp20.c: `value - 0xe000` を step とする)。
    Range { min: u16, max: u16, step: u16 },
}

impl Hidpp {
    /// 0x0003 DeviceFwVersion: エンティティ数 (func 0) と各エンティティ
    /// (func 1, 引数=index) を読む。出典: hidpp20.py `Hidpp20.get_firmware`。
    pub fn fw_entities(&self) -> HubResult<Option<Vec<FwEntity>>> {
        let Some(info) = self.feature(FEAT_DEVICE_FW_VERSION)? else {
            return Ok(None);
        };
        let count = self.request(info.index, 0x00, &[])?[0] as usize;
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let r = self.request(info.index, 0x01, &[index as u8])?;
            out.push(FwEntity {
                kind: r[0] & 0x0F,
                name: String::from_utf8_lossy(&r[1..4]).into_owned(),
                major: r[4],
                minor: r[5],
                build: u16::from_be_bytes([r[6], r[7]]),
            });
        }
        Ok(Some(out))
    }

    /// 0x0003: 主 FW (kind 0) を `"U1 12.03.B0045"` 形式の文字列にする。
    /// major/minor/build は BCD なので 16 進で表示する (Solaar
    /// `get_firmware` の `"{:02X}.{:02X}.B{:04X}"` と同じ)。
    pub fn main_firmware_string(&self) -> HubResult<Option<String>> {
        let Some(entities) = self.fw_entities()? else {
            return Ok(None);
        };
        let Some(fw) = entities.iter().find(|e| e.kind == 0) else {
            return Ok(None);
        };
        let name = fw.name.trim_end_matches([' ', '\0']);
        Ok(Some(format!(
            "{name} {:02X}.{:02X}.B{:04X}",
            fw.major, fw.minor, fw.build
        )))
    }

    /// 0x0005 DeviceName: 名前長 (func 0) → 16B ずつ分割読み
    /// (func 1, 引数=文字オフセット)。出典: hidpp20.py `Hidpp20.get_name`。
    pub fn device_name(&self) -> HubResult<Option<String>> {
        let Some(info) = self.feature(FEAT_DEVICE_NAME)? else {
            return Ok(None);
        };
        let len = self.request(info.index, 0x00, &[])?[0] as usize;
        let mut bytes = Vec::with_capacity(len);
        while bytes.len() < len {
            let r = self.request(info.index, 0x01, &[bytes.len() as u8])?;
            let take = (len - bytes.len()).min(16);
            bytes.extend_from_slice(&r[..take]);
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// 0x2201 AdjustableDPI: センサー数 (func 0)。
    /// 出典: hidpp20.c `hidpp20_adjustable_dpi_get_count`。
    pub fn sensor_count(&self) -> HubResult<Option<u8>> {
        let Some(info) = self.feature(FEAT_ADJUSTABLE_DPI)? else {
            return Ok(None);
        };
        Ok(Some(self.request(info.index, 0x00, &[])?[0]))
    }

    /// 0x2201: DPI リスト (func 1, 引数=センサー index)。
    /// 応答は [sensor エコー, BE16 値列..., 0x0000 終端]。
    /// 出典: hidpp20.c `hidpp20_adjustable_dpi_get_dpi_list`。
    pub fn sensor_dpi_list(&self, sensor: u8) -> HubResult<Option<DpiList>> {
        let Some(info) = self.feature(FEAT_ADJUSTABLE_DPI)? else {
            return Ok(None);
        };
        let r = self.request(info.index, 0x01, &[sensor])?;
        Ok(Some(parse_dpi_list(&r)))
    }

    /// 0x2201: 現在の DPI と既定 DPI (func 2, 引数=センサー index)。
    /// 応答は [sensor エコー, dpi BE16, default dpi BE16]。
    /// 出典: hidpp20.c `hidpp20_adjustable_dpi_get_dpi`。
    pub fn sensor_dpi(&self, sensor: u8) -> HubResult<Option<(u16, u16)>> {
        let Some(info) = self.feature(FEAT_ADJUSTABLE_DPI)? else {
            return Ok(None);
        };
        let r = self.request(info.index, 0x02, &[sensor])?;
        Ok(Some((
            u16::from_be_bytes([r[1], r[2]]),
            u16::from_be_bytes([r[3], r[4]]),
        )))
    }

    /// 0x2201: DPI 設定 (func 3, 引数=センサー index + DPI BE16)。
    /// プロトコル v0 はエコーしないので、エコー 0 以外で不一致ならエラー。
    /// 出典: hidpp20.c `hidpp20_adjustable_dpi_set_sensor_dpi`。
    pub fn set_sensor_dpi(&self, sensor: u8, dpi: u16) -> HubResult<()> {
        let Some(info) = self.feature(FEAT_ADJUSTABLE_DPI)? else {
            return Err(HubError::Unsupported("AdjustableDPI"));
        };
        let mut params = [0u8; 3];
        params[0] = sensor;
        params[1..3].copy_from_slice(&dpi.to_be_bytes());
        let r = self.request(info.index, 0x03, &params)?;
        let echoed = u16::from_be_bytes([r[1], r[2]]);
        if echoed != 0 && echoed != dpi {
            return Err(HubError::Protocol(format!(
                "DPI echo mismatch: sent {dpi}, echoed {echoed}"
            )));
        }
        Ok(())
    }

    /// 0x8060 ReportRate: 対応レート一覧 (func 0)。ビットマップ bit n → (n+1) ms。
    /// 出典: hidpp20.c `hidpp20_adjustable_report_rate_get_report_rate_list`。
    pub fn report_rate_list(&self) -> HubResult<Option<Vec<u8>>> {
        let Some(info) = self.feature(FEAT_REPORT_RATE)? else {
            return Ok(None);
        };
        let bitmap = self.request(info.index, 0x00, &[])?[0];
        Ok(Some(parse_rate_bitmap(bitmap)))
    }

    /// 0x8060: 現在のレート (func 1, ms)。
    /// 出典: hidpp20.c `hidpp20_adjustable_report_rate_get_report_rate`。
    pub fn report_rate(&self) -> HubResult<Option<u8>> {
        let Some(info) = self.feature(FEAT_REPORT_RATE)? else {
            return Ok(None);
        };
        Ok(Some(self.request(info.index, 0x01, &[])?[0]))
    }

    /// 0x8060: レート設定 (func 2, 引数=ms)。
    /// 出典: hidpp20.c `hidpp20_adjustable_report_rate_set_report_rate`。
    pub fn set_report_rate(&self, rate_ms: u8) -> HubResult<()> {
        let Some(info) = self.feature(FEAT_REPORT_RATE)? else {
            return Err(HubError::Unsupported("ReportRate"));
        };
        self.request(info.index, 0x02, &[rate_ms])?;
        Ok(())
    }
}

/// ビットマップ bit n → (n+1) ms の列。
pub(crate) fn parse_rate_bitmap(bitmap: u8) -> Vec<u8> {
    (0..8)
        .filter(|bit| bitmap & (1 << bit) != 0)
        .map(|bit| bit as u8 + 1)
        .collect()
}

/// getSensorDpiList 応答の解釈: 値列と `0xE0xx` 刻み (hidpp20.c:
/// `value > 0xe000` なら step、それ以外は値として (0xE000` なら step、それ以外は値として集める)。
pub(crate) fn parse_dpi_list(r: &[u8; 16]) -> DpiList {
    let mut values = Vec::new();
    let mut step = 0u16;
    let mut index = 1;
    while index + 1 < r.len() {
        let value = u16::from_be_bytes([r[index], r[index + 1]]);
        if value == 0 {
            break;
        }
        if value > 0xE000 {
            step = value - 0xE000;
        } else {
            values.push(value);
        }
        index += 2;
    }
    if step > 0 {
        DpiList::Range {
            min: values.first().copied().unwrap_or(0),
            max: values.last().copied().unwrap_or(0),
            step,
        }
    } else {
        DpiList::Values(values)
    }
}
