//! Logitech HID++ 2.0 プロトコル層 (G502 HERO 対応の土台)。
//!
//! `proto` は素のプロトコル (要求/応答の往復・機能 index のキャッシュ) だけを担う。
//! デバイスドライバとしての実装 (caps / read / apply) はこの上に載る別モジュール。

pub mod driver;
mod driver_buttons;
pub mod driver_read;
pub mod driver_write;
pub mod features;
pub mod onboard_io;
pub mod proto;
#[cfg(test)]
mod tests_driver;
#[cfg(test)]
mod tests_driver_buttons;
#[cfg(test)]
mod tests_driver_buttons_ui;
#[cfg(test)]
mod tests_driver_write;
#[cfg(test)]
mod tests_g502_sectors;
#[cfg(test)]
mod tests_features;
#[cfg(test)]
mod tests_onboard_io;
