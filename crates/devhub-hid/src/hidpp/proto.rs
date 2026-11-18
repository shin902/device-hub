//! HID++ 2.0 の素の要求/応答層。
//!
//! 仕様の参照先: libratbag `src/hidpp20.c` (MIT) と Solaar `lib/logitech_receiver/hidpp20.py`。
//! - 要求: `[report_id, dev_idx, feature_index, (func << 4) | sw_id, params(16B)]`
//!   (hidpp20.c: `hidpp20_request_command_allow_error` — address の上位 4bit が関数番号、
//!   下位 4bit が sw_id)
//! - 応答の照合: report_id (0x10/0x11)・dev_idx・feature_index (sub_id)・
//!   address (func<<4|sw_id) が要求と一致すること (hidpp20.c: `hidpp20_request_command_allow_error`)
//! - エラー応答: HID++ 2.0 は `[rid, idx, 0xFF, feat_idx, func|sw_id, code, ...]`、
//!   HID++ 1.0 は `[0x10, idx, 0x8F, func, code, ...]`
//!   (hidpp20.c: `sub_id == __ERROR_MSG || sub_id == 0xff` の照合、
//!   hidpp-generic.c: `hidpp20_errors` / `hidpp10_errors` のコード表)

use crate::transport::Channel;
use devhub_core::backend::{HubError, HubResult};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 長い報告 (report id 0x11、本体 20B)。
pub const REPORT_ID_LONG: u8 = 0x11;
/// 短い報告 (report id 0x10、本体 7B)。
pub const REPORT_ID_SHORT: u8 = 0x10;
/// 応答側がエラー時に置く sub_id (HID++ 2.0)。
const SUB_ID_ERROR_20: u8 = 0xFF;
/// 応答側がエラー時に置く sub_id (HID++ 1.0)。
const SUB_ID_ERROR_10: u8 = 0x8F;
/// 有線機器の device index (hidpp20.c: `device->index`。ドングルは 0x01..)。
pub const DEV_IDX_WIRED: u8 = 0xFF;
/// 要求に固定で付ける software id (仕様上 0 以外の任意値。hidpp20.c は 0x8 を使用)。
const SW_ID: u8 = 0x0A;
/// 1 回の要求に対する応答待ちの上限。
const RECV_TIMEOUT_MS: i32 = 500;

/// HID++ 2.0 エラーコード → 名 (hidpp-generic.c: `hidpp20_errors`)。
fn hidpp20_error_name(code: u8) -> &'static str {
    match code {
        1 => "Unknown",
        2 => "InvalidArgument",
        3 => "OutOfRange",
        4 => "HWError",
        5 => "LogitechInternal",
        6 => "InvalidFeatureIndex",
        7 => "InvalidFunctionId",
        8 => "Busy",
        9 => "Unsupported",
        _ => "Unknown2.0",
    }
}

/// HID++ 1.0 エラーコード → 名 (hidpp-generic.c: `hidpp10_errors`)。
fn hidpp10_error_name(code: u8) -> &'static str {
    match code {
        1 => "InvalidSubID",
        2 => "InvalidAddress",
        3 => "InvalidValue",
        4 => "ConnectFail",
        5 => "TooManyDevices",
        6 => "AlreadyExists",
        7 => "Busy",
        8 => "UnknownDevice",
        9 => "ResourceError",
        10 => "RequestUnavailable",
        11 => "InvalidParameterValue",
        12 => "WrongPinCode",
        _ => "Unknown1.0",
    }
}

/// 機能 ID → (feature index, type, version) のキャッシュ 1 件。
#[derive(Clone, Copy, Debug)]
pub struct FeatureInfo {
    /// Root getFeature が返した feature index (0 は「無い」)。
    pub index: u8,
    /// 応答の 2 バイト目 (機能の種別ビット)。
    pub feature_type: u8,
    /// 応答の 3 バイト目。
    pub version: u8,
}

/// HID++ 2.0 で 1 デバイスと会話する層。
///
/// Windows では短い報告 (0x10) と長い報告 (0x11) が別の HID コレクションになるため、
/// **長いコレクション (usage page 0xFF00, usage 0x0002) だけを開き、要求は常に 0x11 長報告で送る**
/// (TASK.md の固定設計判断)。`transport::Channel::recv` は生バッファ (buf[0] = report ID)
/// を返すので、ここで番号付きレポートとして解釈する。
pub struct Hidpp {
    pub(crate) ch: Channel,
    dev_idx: u8,
    /// 機能 ID → `FeatureInfo`。「機能が無い」(index 0) も再問い合わせを省くために
    /// 覚えておく (libratbag の `hidpp_root_get_feature_idx` 相当をキャッシュにしたもの)。
    features: Mutex<HashMap<u16, FeatureInfo>>,
    /// 送った要求の数 (診断用。`hidppprobe --timing`)。
    requests: AtomicU32,
}

impl Hidpp {
    /// チャネルと device index から作る (キャッシュ空)。
    pub fn new(ch: Channel, dev_idx: u8) -> Self {
        Self {
            ch,
            dev_idx,
            features: Mutex::new(HashMap::new()),
            requests: AtomicU32::new(0),
        }
    }

    /// これまでに送った要求の数 (1 要求 = 1 往復)。
    pub fn request_count(&self) -> u32 {
        self.requests.load(Ordering::Relaxed)
    }

    /// `feat_index` の機能 `func` を `params` 付きで呼び、応答の params 16B を返す。
    ///
    /// 一致しない受信 (通知・他の sw_id への応答) は読み捨てて待ち続ける。
    /// 全体の待ち時間上限は `RECV_TIMEOUT_MS`。
    pub fn request(&self, feat_index: u8, func: u8, params: &[u8]) -> HubResult<[u8; 16]> {
        let address = (func << 4) | SW_ID;
        let mut body = [0u8; 19];
        body[0] = self.dev_idx;
        body[1] = feat_index;
        body[2] = address;
        let n = params.len().min(16);
        body[3..3 + n].copy_from_slice(&params[..n]);
        self.ch.send_report(REPORT_ID_LONG, &body)?;
        self.requests.fetch_add(1, Ordering::Relaxed);

        let deadline = Instant::now() + Duration::from_millis(RECV_TIMEOUT_MS as u64);
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now()).as_millis() as i32;
            if timeout <= 0 {
                return Err(HubError::Protocol(format!(
                    "HID++ timeout: feat {feat_index:#04x} func {func:#04x}"
                )));
            }
            let buf = self.ch.recv(timeout)?;
            if buf.len() < 4 {
                continue;
            }
            let (rid, idx, sub, addr) = (buf[0], buf[1], buf[2], buf[3]);
            if rid != REPORT_ID_LONG && rid != REPORT_ID_SHORT {
                continue;
            }
            if idx != self.dev_idx {
                continue;
            }
            // HID++ 2.0 エラー: `[rid, idx, 0xFF, feat_idx, func|sw_id, code, ...]`
            // (hidpp20.c: `address == msg->msg.sub_id && parameters[0] == msg->msg.address`)
            if sub == SUB_ID_ERROR_20
                && buf.len() >= 6
                && buf[3] == feat_index
                && buf[4] == address
            {
                let code = buf[5];
                return Err(HubError::Protocol(format!(
                    "HID++ 2.0 error {code}: {}",
                    hidpp20_error_name(code)
                )));
            }
            // HID++ 1.0 エラー: `[0x10, idx, 0x8F, func_sub_id, address, code]`
            // (hidpp20.c: `read_buffer.msg.sub_id == __ERROR_MSG` 照合。
            //  TASK.md 固定設計: 2.0 機に混ざって 1.0 エラーが来る経路)
            if sub == SUB_ID_ERROR_10
                && buf.len() >= 6
                && buf[3] == feat_index
                && buf[4] == address
            {
                let code = buf[5];
                return Err(HubError::Protocol(format!(
                    "HID++ 1.0 error {code}: {}",
                    hidpp10_error_name(code)
                )));
            }
            // 通常応答: sub_id=feature_index、address=func<<4|sw_id。
            // 短い報告 (7B) は params が 3B しか無いが、返り値は 16B に 0 拡張する。
            if sub == feat_index && addr == address {
                let mut out = [0u8; 16];
                if buf.len() > 4 {
                    let n = (buf.len() - 4).min(16);
                    out[..n].copy_from_slice(&buf[4..4 + n]);
                }
                return Ok(out);
            }
        }
    }

    /// Root (index 0) func 0 getFeature。機能が無ければ None。結果はキャッシュする。
    pub fn feature(&self, id: u16) -> HubResult<Option<FeatureInfo>> {
        if let Some(info) = self.features.lock().unwrap().get(&id) {
            return Ok(if info.index == 0 { None } else { Some(*info) });
        }
        let params = [(id >> 8) as u8, (id & 0xFF) as u8];
        let reply = self.request(0, 0x00, &params)?;
        let info = FeatureInfo {
            index: reply[0],
            feature_type: reply[1],
            version: reply[2],
        };
        self.features.lock().unwrap().insert(id, info);
        Ok(if info.index == 0 { None } else { Some(info) })
    }

    /// Root func 1 (ping)。(major, minor) を返す
    /// (hidpp20.c: `hidpp20_root_get_protocol_version`)。
    pub fn protocol_version(&self) -> HubResult<(u8, u8)> {
        let reply = self.request(0, 0x01, &[])?;
        Ok((reply[0], reply[1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock() -> Hidpp {
        Hidpp::new(Channel::mock(), DEV_IDX_WIRED)
    }

    /// `[0x11, idx, feat, addr, ...params]` の応答フレーム。
    fn long_frame(idx: u8, feat: u8, addr: u8, params: &[u8]) -> Vec<u8> {
        let mut v = vec![REPORT_ID_LONG, idx, feat, addr];
        v.extend_from_slice(params);
        v
    }

    /// 送信されるフレーム (report ID + body 19B、末尾 0 埋め)。
    fn sent_frame(idx: u8, feat: u8, addr: u8, params: &[u8]) -> Vec<u8> {
        let mut v = vec![REPORT_ID_LONG, idx, feat, addr];
        v.extend_from_slice(params);
        v.resize(20, 0);
        v
    }

    /// 送信フレームのバイト列: report id: func<<4|sw_id (func 2, sw_id 0x0A → 0x2A)。
    #[test]
    fn request_frame_bytes() {
        let h = mock();
        h.ch.mock_reply(&long_frame(DEV_IDX_WIRED, 3, 0x2A, &[9; 16]));
        let reply = h.request(3, 0x02, &[1, 2, 3]).unwrap();
        assert_eq!(reply, [9u8; 16]);
        let sent = h.ch.mock_sent();
        assert_eq!(sent.len(), 1);
        let expect = sent_frame(DEV_IDX_WIRED, 3, 0x2A, &[1, 2, 3]);
        assert_eq!(sent[0], expect);
    }

    /// 通知 (feat 0 / addr 0x00) と別 sw_id の応答は読み捨てて正しい応答を返す。
    #[test]
    fn request_skips_unrelated_frames() {
        let h = mock();
        // 通知: [0x11, FF, 0x00, 0x00] (feat 0, addr 0)
        h.ch.mock_reply(&[REPORT_ID_LONG, DEV_IDX_WIRED, 0x00, 0x00]);
        // 別 sw_id (0x05) への応答
        h.ch.mock_reply(&long_frame(DEV_IDX_WIRED, 3, 0x25, &[7; 16]));
        // 正しい応答
        h.ch.mock_reply(&long_frame(DEV_IDX_WIRED, 3, 0x2A, &[1; 16]));
        let reply = h.request(3, 0x02, &[]).unwrap();
        assert_eq!(reply, [1u8; 16]);
    }

    /// HID++ 2.0 エラー応答は Err (code 9 → Unsupported)。
    #[test]
    fn request_maps_hidpp20_error() {
        let h = mock();
        h.ch.mock_reply(&[
            REPORT_ID_LONG, DEV_IDX_WIRED, SUB_ID_ERROR_20, 3, 0x2A, 9,
        ]);
        let err = h.request(3, 0x02, &[]).unwrap_err();
        assert!(matches!(err, HubError::Protocol(_)));
        assert!(err.to_string().contains("Unsupported"));
    }

    /// getFeature の結果はキャッシュ: 2 回目は送信しない。
    #[test]
    fn feature_result_is_cached() {
        let h = mock();
        h.ch.mock_reply(&long_frame(DEV_IDX_WIRED, 0, 0x0A, &[5, 0, 1, 0]));
        let f = h.feature(0x2201).unwrap().unwrap();
        assert_eq!(f.index, 5);
        assert_eq!(f.version, 1);
        // 2 回目は通信なしで同じ値
        assert_eq!(h.ch.mock_sent().len(), 1);
        let again = h.feature(0x2201).unwrap().unwrap();
        assert_eq!(again.index, 5);
        assert_eq!(h.ch.mock_sent().len(), 1);
    }
}
