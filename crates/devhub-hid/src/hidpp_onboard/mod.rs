//! HID++ 2.0 (feature 0x8100) onboard memory profile format.
//! Layout of the profile sector and the directory sector follows
//! libratbag (MIT):
//! https://raw.githubusercontent.com/libratbag/libratbag/master/src/hidpp20.c
//!
//! This module does no I/O (HID transport is a separate task).

pub mod buttons;
pub mod crc;
pub mod directory;
pub mod profile;

pub use buttons::{ButtonBinding, SpecialAction};
pub use directory::{parse_directory, serialize_directory, DirectoryEntry, ProfileDirectory};
pub use profile::{LedMode, OnboardProfile, Rgb};
