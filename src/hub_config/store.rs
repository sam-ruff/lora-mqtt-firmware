//! Flash-backed configuration store.
//!
//! Persists the [`HubConfig`] as a single postcard-encoded map entry in the
//! `nvs` data partition (unused by this pure-Rust firmware otherwise), found
//! by reading the partition table at runtime so a custom `partitions.csv`
//! keeps working. sequential-storage wear-levels across the partition's
//! erase sectors.

use core::ops::Range;

use embassy_embedded_hal::adapter::BlockingAsync;
use esp_bootloader_esp_idf::partitions::{
    read_partition_table, DataPartitionSubType, PartitionType,
};
use esp_hal::peripherals::FLASH;
use esp_storage::FlashStorage;
use sequential_storage::cache::{Cache, Uncached};
use sequential_storage::map::{MapConfig, MapStorage};

use super::HubConfig;

/// The single map key the config lives under.
const CONFIG_KEY: u8 = 0;

/// Holds the postcard-encoded key + value, word-aligned. The config is well
/// under this even with every string at its cap.
const BUFFER_SIZE: usize = 512;

type NvsFlash = BlockingAsync<FlashStorage<'static>>;
type NoCache = Cache<Uncached, Uncached, Uncached, u8>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// No partition table or no nvs partition on this flash layout.
    NoNvsPartition,
    /// The nvs partition cannot host a sequential-storage map.
    BadPartition,
    /// A flash read/write/erase failed.
    Flash,
}

pub struct ConfigStore {
    map: MapStorage<u8, NvsFlash, NoCache>,
    buffer: [u8; BUFFER_SIZE],
}

impl ConfigStore {
    /// Take ownership of the flash peripheral, locate the nvs partition and
    /// open the config map inside it.
    pub fn new(flash: FLASH<'static>) -> Result<Self, StoreError> {
        let mut storage = FlashStorage::new(flash);
        let range = find_nvs_range(&mut storage)?;
        let config =
            MapConfig::<NvsFlash>::try_new(range).map_err(|_| StoreError::BadPartition)?;
        Ok(Self {
            map: MapStorage::new(BlockingAsync::new(storage), config, Cache::new_uncached()),
            buffer: [0u8; BUFFER_SIZE],
        })
    }

    /// The stored config, or None when nothing (readable) is stored.
    pub async fn load(&mut self) -> Option<HubConfig> {
        self.map
            .fetch_item::<HubConfig>(&mut self.buffer, &CONFIG_KEY)
            .await
            .ok()
            .flatten()
    }

    pub async fn save(&mut self, config: &HubConfig) -> Result<(), StoreError> {
        self.map
            .store_item(&mut self.buffer, &CONFIG_KEY, config)
            .await
            .map_err(|_| StoreError::Flash)
    }

    pub async fn clear(&mut self) -> Result<(), StoreError> {
        self.map
            .remove_item(&mut self.buffer, &CONFIG_KEY)
            .await
            .map_err(|_| StoreError::Flash)
    }
}

/// Locate the nvs data partition in the on-flash partition table.
fn find_nvs_range(storage: &mut FlashStorage<'static>) -> Result<Range<u32>, StoreError> {
    // 512 bytes holds 16 partition entries; the default table has 3.
    let mut buf = [0u8; 512];
    let table =
        read_partition_table(storage, &mut buf).map_err(|_| StoreError::NoNvsPartition)?;
    let nvs = table
        .find_partition(PartitionType::Data(DataPartitionSubType::Nvs))
        .map_err(|_| StoreError::NoNvsPartition)?
        .ok_or(StoreError::NoNvsPartition)?;
    Ok(nvs.offset()..nvs.offset() + nvs.len())
}
