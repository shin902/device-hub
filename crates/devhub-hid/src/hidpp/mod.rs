//! Logitech HID++ 2.0 プロトコル層 (G502 HERO 対応の土台)。
//!
//! `proto` は素のプロトコル (要求/応答の往復・機能 index のキャッシュ) だけを担う。
//! デバイスドライバとしての実装 (caps / read / apply) はこの上に載る別モジュール。

pub mod proto;
