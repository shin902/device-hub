//! Profile directory sector (sector 0x0000) conversion.
//!
//! One profile per 4 bytes: `[address BE u16][enabled flag][0x00]`.
//! The list ends with an 0xFFFF entry followed by 2 zero bytes.
//! Equivalent to libratbag's `hidpp20_onboard_profiles_write_dict`.

use super::crc;
use devhub_core::backend::{HubError, HubResult};

/// End-of-list marker.
pub const DIRECTORY_END: u16 = 0xffff;
/// Bytes per entry.
pub const ENTRY_BYTES: usize = 4;

/// One directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntry {
    /// Sector address of the profile.
    pub address: u16,
    /// Enabled flag.
    pub enabled: bool,
}

/// The whole directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDirectory {
    /// Entries (both enabled and disabled).
    pub entries: Vec<DirectoryEntry>,
}

/// Parse a directory sector. Stops at the end marker (0xFFFF).
/// Rejects a mismatching trailing CRC.
pub fn parse_directory(bytes: &[u8], sector_size: usize) -> HubResult<ProfileDirectory> {
    if bytes.len() != sector_size || sector_size < ENTRY_BYTES + 2 {
        return Err(HubError::Protocol(format!(
            "invalid directory sector size: {sector_size} bytes (len {})",
            bytes.len()
        )));
    }
    let stored = u16::from_be_bytes([bytes[sector_size - 2], bytes[sector_size - 1]]);
    let calculated = crc::crc_ccitt(&bytes[..sector_size - 2]);
    if stored != calculated {
        return Err(HubError::Protocol(format!(
            "directory CRC mismatch: stored 0x{stored:04x} != calc 0x{calculated:04x}"
        )));
    }
    let mut entries = Vec::new();
    let mut at = 0;
    while at + ENTRY_BYTES <= sector_size {
        let address = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        if address == DIRECTORY_END {
            break;
        }
        entries.push(DirectoryEntry { address, enabled: bytes[at + 2] != 0 });
        at += ENTRY_BYTES;
    }
    Ok(ProfileDirectory { entries })
}

/// Serialize the directory back into a sector. 0xFF padding and CRC.
pub fn serialize_directory(directory: &ProfileDirectory, sector_size: usize) -> Vec<u8> {
    let mut bytes = vec![0xff; sector_size];
    let mut at = 0;
    for entry in &directory.entries {
        if at + ENTRY_BYTES > sector_size {
            break;
        }
        bytes[at..at + 2].copy_from_slice(&entry.address.to_be_bytes());
        bytes[at + 2] = u8::from(entry.enabled);
        bytes[at + 3] = 0x00;
        at += ENTRY_BYTES;
    }
    if at + ENTRY_BYTES <= sector_size {
        bytes[at..at + 2].copy_from_slice(&DIRECTORY_END.to_be_bytes());
        bytes[at + 2] = 0x00;
        bytes[at + 3] = 0x00;
    }
    let checksum = crc::crc_ccitt(&bytes[..sector_size - 2]);
    bytes[sector_size - 2..sector_size].copy_from_slice(&checksum.to_be_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::{parse_directory, serialize_directory, DirectoryEntry, ProfileDirectory, DIRECTORY_END};
    use crate::hidpp_onboard::crc;

    const SECTOR_SIZE: usize = 255;

    fn build_directory_sector() -> Vec<u8> {
        let mut bytes = vec![0xff_u8; SECTOR_SIZE];
        bytes[0..4].copy_from_slice(&[0x00, 0x01, 0x01, 0x00]);
        bytes[4..8].copy_from_slice(&[0x00, 0x02, 0x00, 0x00]);
        bytes[8..12].copy_from_slice(&[0x00, 0x03, 0x01, 0x00]);
        bytes[12..14].copy_from_slice(&DIRECTORY_END.to_be_bytes());
        bytes[14..16].copy_from_slice(&[0x00, 0x00]);
        let checksum = crc::crc_ccitt(&bytes[..SECTOR_SIZE - 2]);
        bytes[SECTOR_SIZE - 2..SECTOR_SIZE].copy_from_slice(&checksum.to_be_bytes());
        bytes
    }

    #[test]
    fn directory_roundtrip_preserves_entries_and_padding() {
        let sector = build_directory_sector();
        let parsed = parse_directory(&sector, SECTOR_SIZE).unwrap();
        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.entries[0], DirectoryEntry { address: 0x0001, enabled: true });
        assert_eq!(parsed.entries[1], DirectoryEntry { address: 0x0002, enabled: false });
        assert_eq!(parsed.entries[2], DirectoryEntry { address: 0x0003, enabled: true });
        let serialized = serialize_directory(&parsed, SECTOR_SIZE);
        assert_eq!(serialized, sector);
    }

    #[test]
    fn directory_rejects_crc_mismatch() {
        let mut sector = build_directory_sector();
        sector[1] = 0x04;
        assert!(parse_directory(&sector, SECTOR_SIZE).is_err());
    }

    #[test]
    fn directory_stops_at_end_marker() {
        let mut sector = build_directory_sector();
        sector[16..20].copy_from_slice(&[0x00, 0x09, 0x01, 0x00]);
        let checksum = crc::crc_ccitt(&sector[..SECTOR_SIZE - 2]);
        sector[SECTOR_SIZE - 2..SECTOR_SIZE].copy_from_slice(&checksum.to_be_bytes());
        let parsed = parse_directory(&sector, SECTOR_SIZE).unwrap();
        assert_eq!(parsed.entries.len(), 3);
    }

    #[test]
    fn directory_serialize_writes_end_marker_and_padding() {
        let dir = ProfileDirectory {
            entries: vec![DirectoryEntry { address: 0x0001, enabled: true }],
        };
        let serialized = serialize_directory(&dir, SECTOR_SIZE);
        assert_eq!(serialized[0..4], [0x00, 0x01, 0x01, 0x00]);
        assert_eq!(serialized[4..6], DIRECTORY_END.to_be_bytes());
        assert_eq!(serialized[6..8], [0x00, 0x00]);
        assert!(serialized[8..SECTOR_SIZE - 2].iter().all(|&b| b == 0xff));
        let parsed = parse_directory(&serialized, SECTOR_SIZE).unwrap();
        assert_eq!(parsed.entries.len(), 1);
    }

    #[test]
    fn directory_rejects_wrong_length() {
        let sector = build_directory_sector();
        assert!(parse_directory(&sector[..100], SECTOR_SIZE).is_err());
        assert!(parse_directory(&sector, 100).is_err());
    }
}
