use eyre::Result;
use serde::{Deserialize, Serialize};
use sqlx::AnyPool;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug)]
pub struct Database {
    db: AnyPool,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BuoyType {
    SFY,
    OMB,
    Unknown,
}

impl BuoyType {
    pub fn to_str(&self) -> &'static str {
        match self {
            BuoyType::SFY => "sfy",
            BuoyType::OMB => "omb",
            BuoyType::Unknown => "unknown",
        }
    }
}

impl From<&str> for BuoyType {
    fn from(s: &str) -> BuoyType {
        match s {
            "sfy" => BuoyType::SFY,
            "omb" => BuoyType::OMB,
            _ => BuoyType::Unknown,
        }
    }
}

impl Into<String> for BuoyType {
    fn into(self) -> String {
        self.to_str().into()
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum OmbMessageType {
    GPS,
    IMU,
    Thermistor,
    Unknown,
}

impl OmbMessageType {
    pub fn to_str(&self) -> &'static str {
        match self {
            OmbMessageType::GPS => "gps",
            OmbMessageType::IMU => "imu",
            OmbMessageType::Thermistor => "thermistor",
            OmbMessageType::Unknown => "unknown",
        }
    }
}

impl From<&str> for OmbMessageType {
    fn from(s: &str) -> OmbMessageType {
        match s {
            "gps" => OmbMessageType::GPS,
            "imu" => OmbMessageType::IMU,
            "thermistor" => OmbMessageType::Thermistor,
            _ => OmbMessageType::Unknown,
        }
    }
}

impl Into<String> for OmbMessageType {
    fn into(self: Self) -> String {
        self.to_str().into()
    }
}

impl Database {
    pub async fn open(path: &str) -> Result<Database> {
        info!("opening database at: {:?}", path);

        let db = AnyPool::connect(path).await?;

        info!("running db migrations..");
        sqlx::migrate!("./migrations").run(&db).await?;

        Ok(Database { db })
    }

    /// Open buoy.
    pub async fn buoy(&self, dev: &str) -> eyre::Result<Buoy> {
        let dev = percent_encoding::percent_decode_str(dev)
            .decode_utf8_lossy()
            .to_string();
        let buoy = sqlx::query!("SELECT dev, name, buoy_type FROM buoys where dev = ?1", dev)
            .fetch_optional(&self.db)
            .await?;

        let known = buoy.is_some();

        if !known {
            debug!("Unknown buoy: {}", dev);
        }

        let name = buoy.as_ref().map(|b| b.name.clone()).flatten();
        let buoy_type = buoy
            .as_ref()
            .map(|b| b.buoy_type.as_str().into())
            .unwrap_or(BuoyType::Unknown);

        Ok(Buoy {
            dev: String::from(dev),
            known,
            name,
            buoy_type,
            db: self.db.clone().clone(),
        })
    }

    /// Get list of buoys.
    pub async fn buoys(&self) -> eyre::Result<Vec<(String, String, String, String)>> {
        let buoys: Vec<_> = sqlx::query!("SELECT dev, name, buoy_type FROM buoys ORDER BY dev")
            .fetch_all(&self.db)
            .await?
            .iter()
            .map(move |r| {
                (
                    r.dev.clone(),
                    r.name.clone().unwrap_or(String::new()),
                    r.buoy_type.clone(),
                )
            }).collect();

        let mut last = Vec::new();

        for r in &buoys {
            let b = self.buoy(&r.0).await?;
            let e = b.last().await.map_or(String::new(), base64::encode);
            last.push(e);
        }

        let buoys = buoys.into_iter().zip(last).map(|(b, l)| (b.0, b.1, b.2, l)).collect();

        Ok(buoys)
    }

    #[cfg(test)]
    pub async fn temporary() -> Database {
        warn!("create temporary database at in memory");

        Database::open("sqlite::memory:").await.unwrap()
    }
}

#[derive(Debug)]
pub struct Buoy {
    dev: String,
    /// Does the buoy exist in the database already.
    known: bool,
    name: Option<String>,
    buoy_type: BuoyType,
    db: AnyPool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub received: i64,
    pub event: String,
    pub data: Option<Vec<u8>>,
}

impl Buoy {
    /// Append new event to SFY buoy, `name` is parsed serial number of buoy.
    pub async fn append(
        &mut self,
        name: Option<String>,
        event: impl AsRef<Path>,
        received: u64,
        file: Option<String>,
        data: impl AsRef<[u8]>,
    ) -> eyre::Result<()> {
        let data = data.as_ref();
        let event = event.as_ref().to_string_lossy().into_owned();

        self.buoy_type = BuoyType::SFY;

        if let Some(ref name) = name {
            if self.name.as_ref() != Some(&name) {
                debug!("Updating name for: {} to {}", self.dev, name);
                self.known = false;
            }
        }

        if !self.known {
            info!(
                "Updating or inserting buoy from {:?} to {:?}",
                self.name, name
            );

            sqlx::query!(
                "INSERT OR REPLACE INTO buoys (dev, name, buoy_type) VALUES ( ?1, ?2, 'sfy' )",
                self.dev,
                name
            )
            .execute(&self.db)
            .await?;

            self.known = true;
        }

        debug!(
            "buoy (sfy): {} ({:?}): appending event: {:?}, received: {}, size: {}",
            self.dev,
            self.name,
            event,
            received,
            data.len()
        );

        let r = received as i64;
        let file = file.unwrap_or_else(|| "unknown".into());
        sqlx::query!(
            "INSERT INTO events (dev, received, event, message_type, data) VALUES ( ?1, ?2, ?3, ?4, ?5 )",
            self.dev,
            r,
            event,
            file,
            data
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Append to OpenMetBuoy (OMB)
    pub async fn append_omb(
        &mut self,
        account: String,
        received: u64,
        message_type: OmbMessageType,
        data: impl AsRef<[u8]>,
    ) -> eyre::Result<()> {
        let data = data.as_ref();

        self.buoy_type = BuoyType::OMB;

        if !self.known {
            sqlx::query!(
                "INSERT OR REPLACE INTO buoys (dev, buoy_type) VALUES ( ?1, 'omb' )",
                self.dev,
            )
            .execute(&self.db)
            .await?;

            self.known = true;
        }

        debug!(
            "buoy (omb): {}: appending event, account: {:?}, type: {:?}, received: {}, size: {}",
            self.dev,
            account,
            message_type,
            received,
            data.len()
        );

        let message_type = message_type.to_str();
        let r = received as i64;
        sqlx::query!(
            "INSERT INTO omb_events (dev, received, account, message_type, data) VALUES ( ?1, ?2, ?3, ?4, ?5 )",
            self.dev,
            r,
            account,
            message_type,
            data
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn entries(&self) -> Result<Vec<(String, String)>> {
        ensure!(self.known, "No such buoy");

        let events = match self.buoy_type {
            BuoyType::SFY => {
                sqlx::query!(
                    "SELECT received, event, message_type FROM events where dev = ?1 ORDER BY received",
                    self.dev
                )
                .map(|r| (format!("{}-{}", r.received, r.event), r.message_type))
                .fetch_all(&self.db)
                .await?
            }
            BuoyType::OMB => {
                sqlx::query!(
                    "SELECT received, event, message_type FROM omb_events where dev = ?1 ORDER BY received",
                    self.dev
                )
                .map(|r| (format!("{}-{}-{}", r.received, r.event, r.message_type), r.message_type))
                .fetch_all(&self.db)
                .await?
            }
            BuoyType::Unknown => return Err(eyre!("Unknown buoy type")),
        };

        Ok(events)
    }

    /// Get the last received axl.qo entry for the buoy.
    pub async fn last(&self) -> Result<Vec<u8>> {
        ensure!(self.known, "No such buoy");

        let data = match self.buoy_type {
            BuoyType::SFY => sqlx::query!("SELECT data FROM events WHERE dev = ?1 AND (message_type = 'axl.qo' or message_type = '_track.qo') ORDER BY received DESC LIMIT 1", self.dev)
                .fetch_one(&self.db)
                .await?.data,

            BuoyType::OMB => sqlx::query!("SELECT data FROM omb_events WHERE dev = ?1 AND message_type = 'gps' ORDER BY received DESC LIMIT 1", self.dev)
                .fetch_one(&self.db)
                .await?.data,

            BuoyType::Unknown => return Err(eyre!("Unknown buoy type"))
        };

        match data {
            Some(data) => Ok(data),
            None => Err(eyre!("No axl entry found.")),
        }
    }

    pub async fn get(&self, file: impl AsRef<Path>) -> Result<Vec<u8>> {
        ensure!(self.known, "No such buoy");

        let file = file.as_ref().to_string_lossy().into_owned();

        let data = match self.buoy_type {
            BuoyType::SFY => {
                let (received, file) = file
                    .split_once('-')
                    .ok_or(eyre!("incorrect format of event"))?;

                sqlx::query!(
                    "SELECT data FROM events WHERE dev = ?1 AND received = ?2 AND event = ?3",
                    self.dev,
                    received,
                    file
                )
                .fetch_one(&self.db)
                .await?
                .data
            }
            BuoyType::OMB => {
                let parts: Vec<_> = file.splitn(3, '-').collect();
                ensure!(parts.len() == 3, "incorrect format of event");
                let received = parts[0];
                let file = parts[1];
                let message_type = parts[2];

                sqlx::query!(
                    "SELECT data FROM omb_events WHERE dev = ?1 AND received = ?2 AND event = ?3 AND message_type = ?4",
                    self.dev,
                    received,
                    file,
                    message_type
                )
                .fetch_one(&self.db)
                .await?.data
            }
            _ => return Err(eyre!("Unknown buoy type")),
        };

        match data {
            Some(data) => Ok(data),
            None => Err(eyre!("No such event found.")),
        }
    }

    pub async fn list_range(&self, start: i64, end: i64) -> Result<Vec<(i64, String, String)>> {
        ensure!(self.known, "No such buoy");

        let events = match self.buoy_type {
            BuoyType::SFY => {
                sqlx::query!(
                    "SELECT event, received, message_type FROM events WHERE dev = ?1 AND received >= ?2 AND received <= ?3 ORDER BY received",
                    self.dev,
                    start,
                    end,
                )
                .map(|r| (r.received, r.event, r.message_type))
                .fetch_all(&self.db)
                .await?
            },
            BuoyType::OMB => {
                sqlx::query!(
                    "SELECT event, message_type, received FROM omb_events WHERE dev = ?1 AND received >= ?2 AND received <= ?3 ORDER BY received",
                    self.dev,
                    start,
                    end,
                )
                .map(|r| (r.received, format!("{}-{}", r.event, r.message_type), r.message_type))
                .fetch_all(&self.db)
                .await?
            },
            BuoyType::Unknown => return Err(eyre!("Unknown buoy type"))
        };

        Ok(events)
    }

    pub async fn get_range(&self, start: i64, end: i64) -> Result<Vec<Event>> {
        ensure!(self.known, "No such buoy");

        let events = match self.buoy_type {
            BuoyType::SFY => {
                sqlx::query_as!(
                    Event,
                    "SELECT event, received, data FROM events WHERE dev = ?1 AND received >= ?2 AND received <= ?3 ORDER BY received",
                    self.dev,
                    start,
                    end,
                )
                .fetch_all(&self.db)
                .await?
            },
            BuoyType::OMB => {
                sqlx::query!(
                    "SELECT event, message_type, received, data FROM omb_events WHERE dev = ?1 AND received >= ?2 AND received <= ?3 ORDER BY received",
                    self.dev,
                    start,
                    end,
                )
                .map(|r| Event { event: format!("{}-{}", r.event, r.message_type), received: r.received, data: r.data })
                .fetch_all(&self.db)
                .await?
            },
            BuoyType::Unknown => return Err(eyre!("Unknown buoy type"))
        };

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_temporary() {
        let _db = Database::temporary().await;
    }

    #[tokio::test]
    async fn get_new_buoy() {
        let db = Database::temporary().await;
        let _b = db.buoy("test-01").await;
    }

    #[tokio::test]
    async fn add_some_entries() {
        let db = Database::temporary().await;
        let mut b = db.buoy("buoy-01").await.unwrap();

        b.append(None, "entry-0", 0, None, "data-0").await.unwrap();
        b.append(None, "entry-1", 0, None, "data-1").await.unwrap();

        assert_eq!(b.get("0-entry-0").await.unwrap(), b"data-0");
    }

    #[tokio::test]
    async fn add_existing_entry() {
        let db = Database::temporary().await;
        let mut b = db.buoy("buoy-01").await.unwrap();

        b.append(None, "entry-0", 0, None, "data-0").await.unwrap();
        assert!(b.append(None, "entry-0", 0, None, "data-1").await.is_err());
    }

    #[tokio::test]
    async fn list_buoys() {
        let db = Database::temporary().await;

        let mut b = db.buoy("buoy-01").await.unwrap();
        b.append(None, "entry-0", 0, None, "data-0").await.unwrap();

        let mut b = db.buoy("buoy-02").await.unwrap();
        b.append(None, "entry-1", 0, None, "data-1").await.unwrap();

        let devs = db.buoys().await.unwrap();
        let devs: Vec<_> = devs.iter().map(|(dev, _, _, _)| dev).collect();

        assert_eq!(devs, ["buoy-01", "buoy-02"]);
    }

    #[tokio::test]
    async fn list_entries() {
        let db = Database::temporary().await;
        let mut b = db.buoy("buoy-01").await.unwrap();
        b.append(None, "entry-0", 0, None, "data-0").await.unwrap();
        b.append(None, "entry-1", 0, None, "data-1").await.unwrap();

        assert_eq!(
            db.buoy("buoy-01").await.unwrap().entries().await.unwrap(),
            [("0-entry-0".into(), "unknown".into()), ("0-entry-1".into(), "unknown".into())]
        );
    }

    #[tokio::test]
    async fn append_get() {
        let db = Database::temporary().await;
        let mut b = db.buoy("buoy-01").await.unwrap();
        b.append(None, "entry-0", 0, None, "data-0").await.unwrap();

        assert_eq!(b.get("0-entry-0").await.unwrap(), b"data-0");
    }

    #[tokio::test]
    async fn append_get_range() {
        let db = Database::temporary().await;
        let mut b = db.buoy("buoy-01").await.unwrap();
        b.append(None, "entry-0", 0, None, "data-0").await.unwrap();
        b.append(None, "entry-1", 1, None, "data-1").await.unwrap();
        b.append(None, "entry-2", 2, None, "data-2").await.unwrap();
        b.append(None, "entry-3", 3, None, "data-3").await.unwrap();

        assert_eq!(b.get("0-entry-0").await.unwrap(), b"data-0");

        let r1 = b.get_range(0, 2).await.unwrap();
        assert_eq!(r1.len(), 3);

        let Event {
            received,
            event,
            data,
        } = &r1[0];
        assert_eq!(*received, 0);
        assert_eq!(event, "entry-0");
        assert_eq!(data, &Some(b"data-0".to_vec()));

        let Event {
            received,
            event,
            data,
        } = &r1[2];
        assert_eq!(*received, 2);
        assert_eq!(event, "entry-2");
        assert_eq!(data, &Some(b"data-2".to_vec()));

        let r2 = b.get_range(2, 3).await.unwrap();
        assert_eq!(r2.len(), 2);

        let Event {
            received,
            event,
            data,
        } = &r2[0];
        assert_eq!(*received, 2);
        assert_eq!(event, "entry-2");
        assert_eq!(data, &Some(b"data-2".to_vec()));

        let Event {
            received,
            event,
            data,
        } = &r2[1];
        assert_eq!(*received, 3);
        assert_eq!(event, "entry-3");
        assert_eq!(data, &Some(b"data-3".to_vec()));
    }

    #[tokio::test]
    async fn append_last() {
        let db = Database::temporary().await;
        let mut b = db.buoy("buoy-01").await.unwrap();
        b.append(None, "entry-0-axl.qo", 0, Some("axl.qo".into()), "data-0").await.unwrap();
        b.append(None, "entry-1-sessi.qo", 0, None, "data-1")
            .await
            .unwrap();

        assert_eq!(b.last().await.unwrap(), b"data-0");
    }

    #[tokio::test]
    async fn append_omb_last() {
        let db = Database::temporary().await;
        let mut b = db.buoy("buoy 01-omb").await.unwrap();
        b.append_omb("testacc".into(), 0, OmbMessageType::GPS, "data-0")
            .await
            .unwrap();
        b.append_omb("testacc".into(), 1, OmbMessageType::GPS, "data-1")
            .await
            .unwrap();

        assert_eq!(b.last().await.unwrap(), b"data-1");
    }
}
