//! Logitech HID++ 2.0 プロトコル層 (G502 HERO 対応の土台)。
//!
//! `proto` は素のプロトコル (要求/応答の往復・機能 index のキャッシュ) だけを担う。
//! デバイスドライバとしての実装 (caps / read / apply) はこの上に載る別モジュール。

pub mod features;
pub mod onboard_io;
pub mod proto;
#[cfg(test)]
mod tests_features;
#[cfg(test)]
mod tests_onboard_io;
