//! HID++ 2.0 OnboardProfiles (0x8100) raw byte I/O.
//!
//! Profile byte format (parse/serialize) is handled by hidpp_onboard
//! (separate task); this module only provides sector-level raw byte
//! round-trips. Feature index is resolved via `Hidpp::feature()`;
//! missing feature -> read `Ok(None)`, write ops -> `HubError::Unsupported`.

use super::proto::Hidpp;
use devhub_core::backend::{HubError, HubResult};

/// OnboardProfiles (hidpp20.h: `HIDPP_PAGE_ONBOARD_PROFILES`).
const FEAT_ONBOARD_PROFILES: u16 = 0x8100;

/// getProfilesDescription response
/// (hidpp20.h: `struct hidpp20_onboard_profiles_info`; 5 reserved bytes dropped).
#[derive(Debug, PartialEq)]
pub struct OnboardProfilesDesc {
    /// Memory model (1 = G402 type; hidpp20.c: `HIDPP20_ONBOARD_PROFILES_MEMORY_TYPE_G402`).
    pub memory_model_id: u8,
    pub profile_format_id: u8,
    pub macro_format_id: u8,
    pub profile_count: u8,
    /// Number of ROM profiles selectable out-of-bounds.
    pub profile_count_oob: u8,
    pub button_count: u8,
    pub sector_count: u8,
    pub sector_size: u16,
    pub mechanical_layout: u8,
    pub various_info: u8,
}

impl Hidpp {
    /// getProfilesDescription (func 0).
    /// Source: hidpp20.c `hidpp20_onboard_profiles_get_profiles_desc`.
    pub fn onboard_profiles_desc(&self) -> HubResult<Option<OnboardProfilesDesc>> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Ok(None);
        };
        let r = self.request(info.index, 0x00, &[])?;
        Ok(Some(OnboardProfilesDesc {
            memory_model_id: r[0],
            profile_format_id: r[1],
            macro_format_id: r[2],
            profile_count: r[3],
            profile_count_oob: r[4],
            button_count: r[5],
            sector_count: r[6],
            sector_size: u16::from_be_bytes([r[7], r[8]]),
            mechanical_layout: r[9],
            various_info: r[10],
        }))
    }

    /// getOnboardMode (func 2).
    /// Source: hidpp20.c `hidpp20_onboard_profiles_get_onboard_mode`.
    pub fn onboard_mode(&self) -> HubResult<Option<u8>> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Ok(None);
        };
        Ok(Some(self.request(info.index, 0x02, &[])?[0]))
    }

    /// setOnboardMode (func 1).
    /// Source: hidpp20.c `hidpp20_onboard_profiles_set_onboard_mode`.
    pub fn set_onboard_mode(&self, mode: u8) -> HubResult<()> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Err(HubError::Unsupported("OnboardProfiles"));
        };
        self.request(info.index, 0x01, &[mode])?;
        Ok(())
    }

    /// getCurrentProfile (func 4). Response params[1] is the active profile
    /// index (params[0] is an echo of unknown purpose).
    /// Source: hidpp20.c `hidpp20_onboard_profiles_get_current_profile`.
    pub fn current_profile(&self) -> HubResult<Option<u8>> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Ok(None);
        };
        Ok(Some(self.request(info.index, 0x04, &[])?[1]))
    }

    /// setCurrentProfile ((functions 3). 0-based index is sent as index + 1.
    /// Source: hidpp20.c `hidpp20_onboard_profiles_set_current_profile`
    /// (`.msg.parameters[1] = index + 1`).
    pub fn set_current_profile(&self, index: u8) -> HubResult<()> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Err(HubError::Unsupported("OnboardProfiles"));
        };
        self.request(info.index, 0x03, &[0, index + 1])?;
        Ok(())
    }

    /// memoryRead (func 5). [sector BE16, offset BE16] -> 16 bytes at that position.
    /// Source: hidpp20.c `hidpp20_onboard_profiles_read_sector`.
    pub fn memory_read(&self, sector: u16, offset: u16) -> HubResult<[u8; 16]> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Err(HubError::Unsupported("OnboardProfiles"));
        };
        let mut params = [0u8; 4];
        params[..2].copy_from_slice(&sector.to_be_bytes());
        params[2..].copy_from_slice(&offset.to_be_bytes());
        self.request(info.index, 0x05, &params)
    }

    /// memoryAddrWrite (func 6). Write destination and length:
    /// [sector BE16, sub_address BE16, count BE16].
    /// Source: hidpp20.c `hidpp20_onboard_profiles_write_start`.
    pub fn memory_addr_write(&self, sector: u16, sub_address: u16, count: u16) -> HubResult<()> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Err(HubError::Unsupported("OnboardProfiles"));
        };
        let mut params = [0u8; 6];
        params[..2].copy_from_slice(&sector.to_be_bytes());
        params[2..4].copy_from_slice(&sub_address.to_be_bytes());
        params[4..].copy_from_slice(&count.to_be_bytes());
        self.request(info.index, 0x06, &params)?;
        Ok(())
    }

    /// memoryWrite (func 7). Streams 16 bytes at the position set by AddrWrite.
    /// Source: hidpp20.c `hidpp20_onboard_profiles_write_data`.
    pub fn memory_write(&self, data: &[u8; 16]) -> HubResult<()> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Err(HubError::Unsupported("OnboardProfiles"));
        };
        self.request(info.index, 0x07, data)?;
        Ok(())
    }

    /// memoryWriteEnd (func 8). Closes the write session (persistence is
    /// committed here).
    /// Source: hidpp20.c `hidpp20_onboard_profiles_write_end`.
    pub fn memory_write_end(&self) -> HubResult<()> {
        let Some(info) = self.feature(FEAT_ONBOARD_PROFILES)? else {
            return Err(HubError::Unsupported("OnboardProfiles"));
        };
        self.request(info.index, 0x08, &[])?;
        Ok(())
    }

    /// Reads a whole sector, 16 bytes per memoryRead. When fewer than 16
    /// bytes remain, the offset is read at `size - 16` so the last chunk
    /// does not run past the sector end (firmware rejects reads past
    /// sector_size - 16 with ERR_INVALID_ARGUMENT).
    /// Source: hidpp20.c `hidpp20_onboard_profiles_read_sector`.
    pub fn read_sector(&self, sector: u16, size: u16) -> HubResult<Vec<u8>> {
        if size < 16 {
            return Err(HubError::Protocol(format!("sector size {size} < 16")));
        }
        let mut data = vec![0u8; size as usize];
        let mut offset: u16 = 0;
        while offset < size {
            let read_at = if size - offset < 16 { size - 16 } else { offset };
            let chunk = self.memory_read(sector, read_at)?;
            data[read_at as usize..read_at as usize + 16].copy_from_slice(&chunk);
            offset += 16;
        }
        Ok(data)
    }

    /// Writes a whole sector: memoryAddrWrite (sub_address 0, count = size)
    /// then 16-byte memoryWrite chunks, then memoryWriteEnd. A
    /// final partial chunk is zero-padded to 16 bytes (sector_size need not
    /// be a multiple of 16; G502 HERO reports 256 via getProfilesDescription).
    /// The caller computes and stores the CRC in the last 2 bytes beforehand
    /// (libratbag does CRC here, but that is `hidpp_onboard`'s job in this
    /// codebase).
    /// Source: hidpp20.c `hidpp20_onboard_profiles_write_sector`.
    pub fn write_sector(&self, sector: u16, data: &[u8]) -> HubResult<()> {
        if data.is_empty() {
            return Err(HubError::Protocol("sector write size 0".into()));
        }
        self.memory_addr_write(sector, 0, data.len() as u16)?;
        for chunk in data.chunks(16) {
            let mut block = [0u8; 16];
            block[..chunk.len()].copy_from_slice(chunk);
            self.memory_write(&block)?;
        }
        self.memory_write_end()
    }
}
