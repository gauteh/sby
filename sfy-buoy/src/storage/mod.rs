//! Stores data-packages to the SD-card.
//!
//! Every data-package is stored to the SD-card and queued for the Notecard. It should also be
//! possible to request a range of old packages.
//!
//! The maximum number of files in a FAT32 directory is 65536. If a data package has ID
//! `1234567` it is put in the file: `12345.X` where `X` is the version of the storage format
//! starting with 1. The packages are serialized using the `postcard` format and separated with
//! `COBS`es. The collection file is the full ID stripped of the last 2 digits. Each collection
//! file holds 100 packages.
//!
//! At 52 Hz and 1024 length data-package, there is 4389 packages per day. That is about 44 collections per day. See tests for more details.

use core::fmt::Debug;
use core::sync::atomic::Ordering;
use cortex_m::interrupt::free;
use embedded_hal::{
    blocking::delay::DelayUs,
    blocking::spi::{Write as DefaultWrite, Transfer},
    digital::v2::OutputPin,
};
use embedded_sdmmc::{
    Error as GenericSdMmcError, Mode, SdCard, SdCardError, VolumeIdx, VolumeManager,
};
use heapless::{String, Vec};

use crate::axl::{self, AxlPacket, AXL_POSTCARD_SZ};
use crate::waves::AxlPacketT;

#[cfg(feature = "raw")]
use crate::waves::RAW_AXL_BYTE_SZ;

#[cfg(feature = "raw")]
pub const PACKAGE_SZ: usize = AXL_POSTCARD_SZ + RAW_AXL_BYTE_SZ;

#[cfg(not(feature = "raw"))]
pub const PACKAGE_SZ: usize = AXL_POSTCARD_SZ;

pub mod clock;
// mod handles;

use clock::CountClock;
// use handles::*;

/// Writing to a file seems to take longer time when it has more packages, this can cause timeouts
/// in the interrupt that drains the IMU FIFO. See <https://github.com/gauteh/sfy/issues/77>.
pub const COLLECTION_SIZE: u32 = 100;
pub const DIR_SIZE: u32 = 1000;
pub const STORAGE_VERSION: u32 = axl::VERSION;
#[cfg(not(feature = "target-test"))]
pub const STORAGE_VERSION_STR: &'static str = "6";

#[cfg(feature = "target-test")]
pub const STORAGE_VERSION_STR: &'static str = "t";

#[derive(Debug, defmt::Format)]
pub enum StorageErr {
    SdMmcErr(SdCardError),
    GenericSdMmmcErr(GenericSdMmcError<SdCardError>),
    ParseIDFailure,
    WriteIDFailure,
    WriteError,
    ReadPackageError,
    SerializationError,
    DiskFull,
    Uninitialized,
}

impl From<SdCardError> for StorageErr {
    fn from(e: SdCardError) -> Self {
        StorageErr::SdMmcErr(e)
    }
}

impl From<embedded_sdmmc::Error<SdCardError>> for StorageErr {
    fn from(e: embedded_sdmmc::Error<SdCardError>) -> Self {
        StorageErr::GenericSdMmmcErr(e)
    }
}

const SD_RETRY_DELAY: i32 = 10 * 60;

enum SdState {
    Uninitialized,
    Retry { last_try: i32 },
    Initialized { next_id: u32 },
}

pub enum SdSpiSpeed {
    Low,
    High,
}

pub struct Storage<Spi: Transfer<u8> + DefaultWrite<u8>, CS: OutputPin, DL: DelayUs<u8>>
where
    <Spi as Transfer<u8>>::Error: Debug,
    <Spi as DefaultWrite<u8>>::Error: Debug,
{
    sd: VolumeManager<SdCard<Spi, CS, DL>, CountClock>,

    reclock_cb: fn(&mut Spi, SdSpiSpeed) -> (),
    clock: CountClock,
    state: SdState,
}

impl<Spi: Transfer<u8> + DefaultWrite<u8>, CS: OutputPin, DL: DelayUs<u8>> Storage<Spi, CS, DL>
where
    <Spi as Transfer<u8>>::Error: Debug,
    <Spi as DefaultWrite<u8>>::Error: Debug,
{
    /// Returns an un-initialized storage module.
    pub fn open(
        spi: Spi,
        cs: CS,
        clock: CountClock,
        reclock_cb: fn(&mut Spi, SdSpiSpeed) -> (),
        delay: DL,
    ) -> Storage<Spi, CS, DL> {
        defmt::info!("Opening SD card..");

        let sd = SdCard::new(spi, cs, delay);
        let sd = VolumeManager::new(sd, clock);

        Storage {
            sd,
            reclock_cb,
            clock,
            state: SdState::Uninitialized,
        }
    }

    pub fn acquire(&mut self) -> Result<SdHandle<'_, Spi, CS, DL>, StorageErr> {
        SdHandle::acquire(self)
    }

    /// Returns the next free ID.
    pub fn next_id(&self) -> Option<u32> {
        match self.state {
            SdState::Initialized { next_id } => Some(next_id),
            _ => None,
        }
    }

    pub fn deinit(&mut self) {
        self.state = SdState::Uninitialized;
    }

    /// Deserialize and return AxlPacket.
    pub fn get(&mut self, id: u32) -> Result<AxlPacket, StorageErr> {
        defmt::debug!("Reading file: {}", id);
        let (dir, collection, file, offset) = id_to_parts(id);

        let mut buf: Vec<u8, { AXL_POSTCARD_SZ }> = Vec::new();
        buf.resize_default(AXL_POSTCARD_SZ).unwrap();

        defmt::debug!(
            "Reading package id: {} from collection: {}, fileid: {}, offset: {}",
            id,
            collection,
            file,
            offset
        );

        let sz = self
            .acquire()
            .and_then(|mut sd| sd.read(dir, &collection, offset, &mut buf))?;

        defmt::trace!("Read {:?} bytes.", sz);

        // De-serialize
        let pck: AxlPacket =
            postcard::from_bytes_cobs(&mut buf).map_err(|_| StorageErr::ReadPackageError)?;

        Ok(pck)
    }

    /// Store a new package.
    pub fn store(&mut self, pck: &mut AxlPacketT) -> Result<u32, StorageErr> {
        #[cfg(feature = "raw")]
        let (pck, raw) = pck;

        #[cfg(not(feature = "raw"))]
        let (pck,) = pck;

        let mut sd = self.acquire()?;

        // If writing fails we will always start a new collection, so ID's should not get out of
        // sync within one collection file.
        let id = sd.advance_id()?;
        let (dir, collection, fid, offset) = id_to_parts(id);

        // Package now has a storage ID.
        pck.storage_id = Some(id);

        // Serialize
        let mut buf: Vec<u8, { AXL_POSTCARD_SZ }> = postcard::to_vec_cobs(pck)
            .inspect_err(|e| defmt::error!("Serialization: {:?}", defmt::Debug2Format(e)))
            .map_err(|_| StorageErr::SerializationError)?;
        buf.resize_default(buf.capacity()).unwrap();

        // Serialize raw bytes
        #[cfg(feature = "raw")]
        let raw_bytes = {
            #[cfg(target_endian = "big")]
            compile_error!("serializied samples are assumed to be in little endian, target platform is big endian and no conversion is implemented.");
            raw.resize_default(raw.capacity()).unwrap();
            let raw_bytes: &[u8] = bytemuck::cast_slice(&raw.as_slice());
            debug_assert_eq!(raw_bytes.len(), RAW_AXL_BYTE_SZ);

            raw_bytes
        };

        #[cfg(not(feature = "raw"))]
        let raw_bytes: &[u8] = &[];

        // And write..
        defmt::info!(
            "Writing package to card id: {}, size: {} + {}, timestamp: {}, collection: {}, fileid: {}, offset: {}",
            id,
            buf.len(),
            raw_bytes.len(),
            pck.timestamp,
            collection,
            fid,
            offset
        );

        sd.write(dir, &collection, &buf, raw_bytes)?;
        defmt::debug!("Package written.");

        Ok(id)
    }
}

pub struct SdHandle<'a, Spi: Transfer<u8> + DefaultWrite<u8>, CS: OutputPin, DL: DelayUs<u8>>
where
    <Spi as Transfer<u8>>::Error: Debug,
    <Spi as DefaultWrite<u8>>::Error: Debug,
{
    sd: &'a mut VolumeManager<SdCard<Spi, CS, DL>, CountClock>,
    state: &'a mut SdState,
}

impl<
        'spi,
        Spi: Transfer<u8> + 'spi + DefaultWrite<u8>,
        CS: OutputPin + 'spi,
        DL: DelayUs<u8> + 'spi,
    > SdHandle<'spi, Spi, CS, DL>
where
    <Spi as Transfer<u8>>::Error: Debug,
    <Spi as DefaultWrite<u8>>::Error: Debug,
{
    fn acquire<'a>(
        storage: &'a mut Storage<Spi, CS, DL>,
    ) -> Result<SdHandle<'a, Spi, CS, DL>, StorageErr> {
        match storage.state {
            SdState::Retry { last_try } => {
                let now = storage.clock.0.load(Ordering::Relaxed);
                if (now - last_try) > SD_RETRY_DELAY {
                    defmt::info!("Ready to re-try SD-card initialization.");
                    storage.state = SdState::Uninitialized;
                    Self::acquire(storage)
                } else {
                    defmt::debug!(
                        "Waiting to re-try sd-card ({} - {} = {})..",
                        now,
                        last_try,
                        (now - last_try)
                    );
                    Err(StorageErr::Uninitialized)
                }
            }
            SdState::Uninitialized => {
                defmt::info!("Initializing SD-card (low-speed)..");
                storage.state = SdState::Retry {
                    last_try: storage.clock.0.load(Ordering::Relaxed),
                };
                storage
                    .sd
                    .device()
                    .spi(|spi| (storage.reclock_cb)(spi, SdSpiSpeed::Low));

                // XXX: This is slow if it fails (time-out), hopefully not too slow, but if so
                // needs to only be attempted seldomly.
                // let mut block = storage.sd.acquire()?;
                storage.sd.device().mark_card_uninit();

                let sz = storage.sd.device().num_bytes()? / 1024_u64.pow(2);
                defmt::info!("SD card size: {} mb", sz);

                defmt::debug!("Increasing SPI speed.");
                storage
                    .sd
                    .device()
                    .spi(|spi| (storage.reclock_cb)(spi, SdSpiSpeed::High));

                // XXX: This is a slow operation which is likely to cause trouble if it is done on
                // every send to notecard loop. Hopefully we will fail above (quickly
                // enough), otherwise this can only be attempted seldomly.
                let next_id =
                    Self::find_first_free_collection(&mut storage.sd, None)? * COLLECTION_SIZE;
                defmt::info!("Next free ID: {}", next_id);

                storage.state = SdState::Initialized { next_id };

                Ok(SdHandle {
                    sd: &mut storage.sd,
                    state: &mut storage.state,
                })
            }
            SdState::Initialized { next_id: _ } => Ok(SdHandle {
                sd: &mut storage.sd,
                state: &mut storage.state,
            }),
        }
    }

    pub fn write(
        &mut self,
        dir: u32,
        collection: &str,
        buf: &[u8],
        #[allow(unused)] buf_raw: &[u8],
    ) -> Result<(), StorageErr> {
        let sz: Result<(), StorageErr> = try {
            let mut v = self.sd.open_volume(VolumeIdx(0))?;
            let mut root = v.open_root_dir()?;
            let mut dir = root.open_dir(dir_name(dir).as_str())?;
            let mut f = dir.open_file_in_dir(collection, Mode::ReadWriteCreateOrAppend)?;
            f.seek_from_end(0)
                .inspect_err(|e| defmt::error!("File seek error: {}", e))
                .map_err(|_| StorageErr::WriteError)?; // We should already be at the
                                                       // end.
            #[cfg(feature = "raw")]
            {
                f.write(&buf)?;
                f.write(&buf_raw)?;
            }

            #[cfg(not(feature = "raw"))]
            f.write(&buf)?;

            ()
        };

        if sz.is_err() {
            *self.state = SdState::Uninitialized;
        }

        sz
    }

    pub fn read(
        &mut self,
        dir: u32,
        collection: &str,
        offset: usize,
        buf: &mut [u8],
    ) -> Result<usize, StorageErr> {
        let sz: Result<usize, StorageErr> = try {
            let mut v = self.sd.open_volume(VolumeIdx(0))?;
            let mut root = v.open_root_dir()?;
            let mut dir = root.open_dir(dir_name(dir).as_str())?;
            let mut f = dir.open_file_in_dir(collection, Mode::ReadOnly)?;

            if f.length() < (offset + AXL_POSTCARD_SZ) as u32 {
                defmt::debug!("Collection is not long enough, no such file in it.");
                return Err(GenericSdMmcError::FileNotFound.into());
            }

            f.seek_from_start(offset as u32)
                .map_err(|_| StorageErr::ReadPackageError)?;
            free(|_| f.read(buf))?
        };

        if sz.is_err() {
            *self.state = SdState::Uninitialized;
        }

        sz
    }

    /// Get the next free ID (and advance to new collection if necessary).
    fn advance_id(&mut self) -> Result<u32, StorageErr> {
        if let SdState::Initialized { next_id: id } = &mut self.state {
            let current = *id;
            let mut next_id = *id + 1;

            // Check that the next collection is free, if rolling over.
            if next_id % COLLECTION_SIZE == 0 {
                let c = next_id / COLLECTION_SIZE;
                let nc = Self::find_first_free_collection(&mut self.sd, Some(c))?;

                if nc > c {
                    defmt::info!("Starting new collection: {}", c);
                    next_id = nc * COLLECTION_SIZE;
                }
            }

            *id = next_id;
            Ok(current)
        } else {
            Err(StorageErr::Uninitialized)
        }
    }

    /// Find the first free collection. Every time the buoy starts up a new collection will be used
    /// to prevent offset mismatch between file id. The ID will be set to the first entry in that
    /// collection.
    pub fn find_first_free_collection<'a>(
        sd: &'a mut VolumeManager<SdCard<Spi, CS, DL>, CountClock>,
        start: Option<u32>,
    ) -> Result<u32, StorageErr> {
        let mut v = sd.open_volume(VolumeIdx(0))?;
        let mut root = v.open_root_dir()?;

        let dstart = start.unwrap_or(0) / DIR_SIZE;
        for d in dstart..65536u32 {
            let dir = dir_name(d);

            match root.find_directory_entry(dir.as_str()) {
                Ok(_) => continue,
                Err(GenericSdMmcError::FileNotFound) => {
                    if d == 0 {
                        return Ok(0);
                    } else {
                        // Look for the last collection in the previous directory.
                        let d = d - 1;
                        let start = d * DIR_SIZE;
                        let dir = dir_name(d);
                        let mut dir = root.open_dir(dir.as_str())?; // must exist

                        for c in start..(start + DIR_SIZE) {
                            let f = collection_fname(c);
                            defmt::debug!("Searching for free collection, testing: {}", f);
                            match dir.find_directory_entry(f.as_str()) {
                                Ok(_) => continue,
                                Err(GenericSdMmcError::FileNotFound) => return Ok(c),
                                Err(e) => return Err(e.into()),
                            }
                        }

                        // This dir was full: take next dir
                        return Ok((d+1)*DIR_SIZE);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }

        Err(StorageErr::DiskFull)
    }

    pub fn remove_collection(&mut self, collection: u32) -> Result<(), StorageErr> {
        defmt::info!("Removing collection: {}", collection);

        let f = collection_fname(collection);

        let mut v = self.sd.open_volume(VolumeIdx(0))?;
        let mut root = v.open_root_dir()?;

        let d = collection / DIR_SIZE;
        let mut dir = root.open_dir(dir_name(d).as_str())?;
        dir.delete_file_in_dir(f.as_str())?;

        Ok(())
    }
}

pub fn dir_name(dir: u32) -> String<32> {
    let mut f: String<32> = String::from(dir);
    f
}

pub fn collection_fname(c: u32) -> String<32> {
    let mut f: String<32> = String::from(c);
    f.push_str(".").unwrap();
    f.push_str(STORAGE_VERSION_STR).unwrap();
    f
}

/// Calculate collection file, file number in collection and byte offset of start of pacakge in
/// collection file for a given ID.
pub fn id_to_parts(id: u32) -> (u32, String<32>, u32, usize) {
    let collection = id / COLLECTION_SIZE;
    let dir = collection / DIR_SIZE;
    let fileid = id % COLLECTION_SIZE;
    let offset = fileid as usize * PACKAGE_SZ;

    let collection = collection_fname(collection);

    (dir, collection, fileid, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axl::AXL_SZ;

    #[test]
    fn version_str() {
        let n: u32 = STORAGE_VERSION_STR.parse().unwrap();
        assert_eq!(n, STORAGE_VERSION);
    }

    #[test]
    fn test_id_to_parts() {
        let (dir, c, file, o) = id_to_parts(0);
        assert_eq!(dir, 0);
        assert_eq!(c, "0.6");
        assert_eq!(file, 0);
        assert_eq!(o, 0);

        let (dir, c, file, o) = id_to_parts(1231255);
        assert_eq!(dir, 12);
        assert_eq!(c, "12312.6");
        assert_eq!(file, 55);
        assert_eq!(o, 55 * PACKAGE_SZ);
    }

    #[test]
    fn test_fat32_limits() {
        let pcks_per_day = 52 * 60 * 60 * 24 / 1024;
        let collection_file_size = COLLECTION_SIZE * PACKAGE_SZ as u32;

        // max file size.
        assert!((collection_file_size as u64) < { 4 * 1024 * 1024 * 1024 });

        println!("collection file size: {} b", collection_file_size);
        println!("pcks per day: {}", pcks_per_day);

        let collections_per_day = pcks_per_day as f32 / COLLECTION_SIZE as f32;
        let collections_per_year = (pcks_per_day * 365) as f32 / COLLECTION_SIZE as f32;
        println!("Collections per day: {}", collections_per_day);
        println!("Collections per year: {}", collections_per_year);
        println!(
            "Years of collections possible (FAT32 file limit): {}",
            65536.0 / collections_per_year
        );

        // max files in directory (should last at least a year)
        assert!(collections_per_year < 65536 as f32);
    }

    #[test]
    #[ignore]
    fn read_synth_collection() {
        let mut c = std::fs::read("tests/data/0.2").unwrap();
        assert_eq!(c.len(), AXL_POSTCARD_SZ * 4);

        let buf = c.as_mut_slice();

        let p0: AxlPacket = postcard::from_bytes_cobs(&mut buf[..AXL_POSTCARD_SZ]).unwrap();
        let p1: AxlPacket =
            postcard::from_bytes_cobs(&mut buf[AXL_POSTCARD_SZ..(2 * AXL_POSTCARD_SZ)]).unwrap();
        let p2: AxlPacket =
            postcard::from_bytes_cobs(&mut buf[(AXL_POSTCARD_SZ * 2)..(AXL_POSTCARD_SZ * 3)])
                .unwrap();

        assert_eq!(p0.storage_id, Some(0));
        assert_eq!(p1.storage_id, Some(1));
        assert_eq!(p2.storage_id, Some(2));

        let p0_truth = AxlPacket {
            timestamp: 1002330,
            position_time: 123123,
            temperature: 0.0,
            lat: 34.52341,
            lon: 54.012,
            freq: 53.0,
            accel_range: 8.,
            gyro_range: 500.,
            offset: 15,
            storage_id: Some(0),
            storage_version: STORAGE_VERSION,
            data: (6..3078).map(|v| v as u16).collect::<Vec<_, { AXL_SZ }>>(),
        };
        let p1_truth = AxlPacket {
            timestamp: 1002400,
            position_time: 123123,
            temperature: 0.0,
            lat: 34.52341,
            lon: 54.012,
            freq: 53.0,
            accel_range: 8.,
            gyro_range: 500.,
            offset: 15,
            storage_id: Some(1),
            storage_version: STORAGE_VERSION,
            data: (6..3078).map(|v| v as u16).collect::<Vec<_, { AXL_SZ }>>(),
        };
        let p2_truth = AxlPacket {
            timestamp: 1002500,
            position_time: 123123,
            temperature: 0.0,
            lat: 34.52341,
            lon: 54.012,
            freq: 53.0,
            accel_range: 8.,
            gyro_range: 500.,
            offset: 15,
            storage_id: Some(2),
            storage_version: STORAGE_VERSION,
            data: (9..3081).map(|v| v as u16).collect::<Vec<_, { AXL_SZ }>>(),
        };

        assert_eq!(p0_truth, p0);
        assert_eq!(p1_truth, p1);
        assert_eq!(p2_truth, p2);
    }

    #[ignore]
    #[test]
    fn read_real_data() {
        let mut c = std::fs::read("tests/data/2.2").unwrap();
        assert_eq!(c.len(), AXL_POSTCARD_SZ * 12);

        let buf = c.as_mut_slice();

        for p in 0..12 {
            let slice = &mut buf[(AXL_POSTCARD_SZ * p)..(AXL_POSTCARD_SZ * (p + 1))];
            let pck: AxlPacket = postcard::from_bytes_cobs(slice).unwrap();
            println!("Deserialized data package: {:?}", pck);
            assert_eq!(pck.storage_id, Some(200 + p as u32));
        }
    }
}
