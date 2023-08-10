use crate::axl::{AxlPacket, AXL_OUTN};
use blues_notecard::{self as notecard, NoteError, Notecard, NotecardConfig};
use core::ops::{Deref, DerefMut};
use embedded_hal::blocking::delay::DelayMs;
use embedded_hal::blocking::i2c::{Read, Write};

use crate::NOTEQ_SZ;

pub const BUOYSN: &str = match option_env!("BUOYSN") {
    Some(v) => v,
    None => "cain",
};

// GPS is sampled at this interval (seconds) when movement is detected by the accelerometer on the
// modem. When below 300 seconds the GPS is not turned off when the buoy is moving. For experiment
// drifting in fjords and similar 10 minutes is sufficient. However, for experiments on beaches a
// higher sample rate is useful.
include!(concat!(env!("OUT_DIR"), "/config.rs"));

/// Initialize sync when storage use is above this percentage.
pub const NOTECARD_STORAGE_INIT_SYNC: u32 = 65;

pub struct Notecarrier<I2C: Read + Write> {
    note: Notecard<I2C>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, defmt::Format, PartialEq)]
pub struct StorageIdInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_id: Option<u32>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, defmt::Format, PartialEq)]
pub struct RequestData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_start: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_end: Option<u32>,
}

impl<I2C: Read + Write> Notecarrier<I2C> {
    pub fn new(i2c: I2C, delay: &mut impl DelayMs<u16>) -> Result<Notecarrier<I2C>, NoteError> {
        let mut note = Notecard::new_with_config(
            i2c,
            NotecardConfig {
                chunk_delay: 5,
                segment_delay: 20,
                ..Default::default()
            },
        );
        note.initialize(delay)?;

        // Location mode is not supported when in continuous mode.
        #[cfg(feature = "continuous")]
        note.card()
            .location_mode(delay, Some("off"), None, None, None, None, None, None, None)?
            .wait(delay)?;

        note.hub()
            .set(
                delay,
                Some(env!("BUOYPR", "Specify notehub project")),
                None,
                if cfg!(feature = "continuous") {
                    Some(notecard::hub::req::HubMode::Continuous)
                } else {
                    Some(notecard::hub::req::HubMode::Periodic)
                },
                Some(BUOYSN),
                Some(40), // max time between out-going sync in minutes.
                None,
                None,
                None,
                None,
                Some(false),
                None,
            )?
            .wait(delay)?;

        #[cfg(not(feature = "continuous"))]
        note.card()
            .location_mode(
                delay,
                Some("periodic"),
                Some(GPS_PERIOD),
                None,
                None,
                None,
                None,
                None,
                None,
            )?
            .wait(delay)?;

        note.card()
            .location_track(delay, true, true, false, Some(1), None)?
            .wait(delay)?;

        let version = note.card().version(delay)?.wait(delay)?;
        defmt::info!("Notecard version: {:?}", version);

        let mut n = Notecarrier { note };
        n.setup_templates(delay)?;

        defmt::info!("initializing initial sync ..");
        n.note.hub().sync(delay, false)?.wait(delay)?;

        Ok(n)
    }

    /// Initiate sync and wait for it to complete (or time out).
    pub fn sync_and_wait(
        &mut self,
        delay: &mut impl DelayMs<u16>,
        timeout_ms: u16,
    ) -> Result<bool, NoteError> {
        defmt::info!("sync..");
        self.note.hub().sync(delay, true)?.wait(delay)?;

        for _ in 0..(timeout_ms / 1000) {
            delay.delay_ms(1000u16);
            defmt::debug!("querying sync status..");
            let status = self.note.hub().sync_status(delay)?.wait(delay);
            defmt::debug!("status: {:?}", status);

            if let Ok(status) = status {
                if status.completed.is_some() {
                    defmt::info!("successful sync.");
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Set up note templates for sensor data and other messages, this will save space and
    /// bandwidth.
    fn setup_templates(&mut self, delay: &mut impl DelayMs<u16>) -> Result<(), NoteError> {
        defmt::debug!("setting up templates..");

        #[derive(serde::Serialize, Default)]
        struct AxlPacketMetaTemplate {
            timestamp: u32,
            offset: u32,

            storage_id: u32,
            storage_version: u32,

            position_time: u32,
            lon: f32,
            lat: f32,
            temperature: f32,

            freq: f32,
            length: u32,
        }

        let meta_template = AxlPacketMetaTemplate {
            timestamp: 18,
            offset: 14,

            storage_id: 14,
            storage_version: 14,

            position_time: 14,
            lon: 18.1,
            lat: 18.1,
            temperature: 18.1,

            freq: 14.1,
            length: 14,
        };

        defmt::debug!("setting up template for AxlPacketMeta");
        self.note()
            .template(
                delay,
                Some("axl.qo"),
                Some(meta_template),
                Some(AXL_OUTN as u32),
            )?
            .wait(delay)?;

        Ok(())
    }

    pub fn send(
        &mut self,
        pck: &AxlPacket,
        delay: &mut impl DelayMs<u16>,
    ) -> Result<usize, NoteError> {
        let (meta, b64) = pck.split();

        let r = self
            .note
            .note()
            .add(
                delay,
                Some("axl.qo"),
                None,
                Some(meta),
                Some(core::str::from_utf8(&b64).unwrap()),
                if cfg!(feature = "continuous") {
                    true
                } else {
                    false
                },
            )?
            .wait(delay)?;

        defmt::info!(
            "Sent data package: {}, bytes: {} (note: {:?})",
            pck.storage_id,
            b64.len(),
            r
        );

        Ok(b64.len())
    }

    /// Send log messages
    pub fn drain_log(
        &mut self,
        queue: &heapless::mpmc::Q4<heapless::String<256>>,
        delay: &mut impl DelayMs<u16>,
    ) -> Result<(), NoteError> {
        while let Some(msg) = queue.dequeue() {
            defmt::info!("logging message: {}", msg);
            self.note
                .hub()
                .log(delay, msg.as_str(), false, false)?
                .wait(delay)?;
        }

        Ok(())
    }

    pub fn read_storage_info(
        &mut self,
        delay: &mut impl DelayMs<u16>,
    ) -> Result<(Option<StorageIdInfo>, Option<RequestData>), NoteError> {
        let r = self
            .note
            .note()
            .get(delay, "storage.dbx", "storage-info", false, false)?
            .wait(delay)
            .map(|r| r.body)
            .unwrap_or(None);

        let d: Option<RequestData> = self
            .note
            .note()
            .get(delay, "storage.db", "request-data", false, false)?
            .wait(delay)
            .map(|r| r.body)
            .unwrap_or(None);

        Ok((r, d))
    }

    pub fn write_storage_info(
        &mut self,
        delay: &mut impl DelayMs<u16>,
        mut sent_id: Option<u32>,
        clear_request: bool,
    ) -> Result<(), NoteError> {
        if clear_request {
            defmt::info!("Clearing data-request..");
            self.note
                .note()
                .delete(delay, "storage.db", "request-data")
                .and_then(|r| r.wait(delay))
                .inspect_err(|e| defmt::error!("Failed to delete request-data: {:?}", e))
                .ok();

            sent_id = None;
        }

        let current_info = self.read_storage_info(delay).ok().map(|(c, _)| c).flatten();

        let info = StorageIdInfo { sent_id };

        if Some(&info) != current_info.as_ref() {
            defmt::trace!(
                "Updating sent_id: {}, clear request: {}",
                sent_id,
                clear_request,
            );
            self.note
                .note()
                .delete(delay, "storage.dbx", "storage-info")
                .and_then(|r| r.wait(delay))
                .inspect_err(|e| defmt::error!("Failed to delete storage-info: {:?}", e))
                .ok();

            self.note
                .note()
                .update(
                    delay,
                    "storage.dbx",
                    "storage-info",
                    Some(info),
                    None,
                    false,
                )?
                .wait(delay)?;
        }

        Ok(())
    }

    /// Send queued packages to the notecard.
    pub fn drain_queue(
        &mut self,
        queue: &mut heapless::spsc::Consumer<'static, AxlPacket, NOTEQ_SZ>,
        delay: &mut impl DelayMs<u16>,
    ) -> Result<usize, NoteError> {
        // Sending packages takes a long time (16-17 seconds). Only 1 package is sent at a time
        // before running main-loop again and letting other tasks run. The main-loop will keep
        // going immediately again if there are more data in the queue.

        let mut tsz = 0;

        while let Some(pck) = queue.dequeue() {
            // #[cfg(not(feature = "continuous"))]
            // {
            //     let sync_status = self.note.hub().sync_status(delay)?.wait(delay)?;

            //     if sync_status.requested.is_some() {
            //         defmt::warn!(
            //             "notecard is syncing, not sending any data-packages until done: queue sz: {}",
            //             queue.len()
            //         );
            //         return Ok(sz);
            //     }
            // }

            // TODO: if status was over 75 last time, don't spam notecard with status requests.
            let status = self.note.card().status(delay)?.wait(delay)?;

            if status.storage > 75 {
                // wait until notecard has synced.
                defmt::warn!("notecard is more than 75% full, not adding more notes until sync is done: queue sz: {}", queue.len());
                return Ok(0);
            }

            defmt::info!(
                "sending package: note queue sz (after dequeue): {}",
                queue.len()
            );
            match self.send(&pck, delay) {
                Ok(sz) => {
                    tsz += sz;
                }
                Err(e) => {
                    defmt::error!(
                        "Error while sending package to notecard: {:?}, retrying..",
                        e
                    );
                    match self.send(&pck, delay) {
                        Ok(sz) => {
                            tsz += sz;
                        }
                        Err(e) => {
                            defmt::error!("Error while sending package to notecard: {:?}, discarding package.", e);
                            return Err(e);
                        }
                    }
                }
            }

            #[cfg(feature = "raw")]
            break;
        }

        Ok(tsz)
    }

    /// Check if notecard is filling up, and initiate sync in that case.
    pub fn check_and_sync(&mut self, delay: &mut impl DelayMs<u16>) -> Result<(), NoteError> {
        let status = self.note.card().status(delay)?.wait(delay)?;
        defmt::trace!("card.status: {}", status);

        let sync_status = self.note.hub().sync_status(delay)?.wait(delay)?;
        defmt::trace!("hub.sync_status: {}", sync_status);

        #[cfg(debug_assertions)]
        {
            let wireless = self.note.card().wireless(delay).and_then(|r| r.wait(delay));
            defmt::trace!("card.wireless: {}", wireless);
        }

        if status.storage > NOTECARD_STORAGE_INIT_SYNC as usize {
            if sync_status.requested.is_none() {
                defmt::warn!(
                    "notecard is more than {}% full, initiating sync.",
                    NOTECARD_STORAGE_INIT_SYNC
                );
                self.note.hub().sync(delay, false)?.wait(delay)?;
            }
            defmt::info!(
                "notecard is filling up ({}%): sync status: {:?}",
                status.storage,
                sync_status
            );
        }

        Ok(())
    }
}

impl<I2C: Read + Write> Deref for Notecarrier<I2C> {
    type Target = Notecard<I2C>;

    fn deref(&self) -> &Self::Target {
        &self.note
    }
}

impl<I2C: Read + Write> DerefMut for Notecarrier<I2C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.note
    }
}

#[cfg(test)]
mod tests {
    use crate::axl::AXL_SZ;
    use half::f16;

    #[test]
    fn read_transmitted_data_package() {
        use std::fs;

        let sent_data = (0..AXL_SZ)
            .map(|v| f16::from_f32(v as f32))
            .collect::<heapless::Vec<_, { AXL_SZ }>>();

        let length: usize = 8192;
        let b64 = fs::read("tests/data/transmitted_payload.txt").unwrap();

        let b64 = &b64[..length];

        // this test assumes host platform is little endian

        let mut buf = Vec::with_capacity(3072 * 2);
        buf.resize(3072 * 2, 0);
        let _data_bytes = base64::decode_config_slice(b64, base64::STANDARD, &mut buf).unwrap();
        let data_values = bytemuck::cast_slice::<_, half::f16>(&buf);

        assert_eq!(sent_data, data_values);
    }
}
