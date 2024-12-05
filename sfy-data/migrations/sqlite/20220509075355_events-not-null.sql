CREATE TEMPORARY TABLE events_backup (dev, event, received, data);
INSERT INTO events_backup SELECT dev, event, received, data FROM events;

DROP TABLE events;

CREATE TABLE IF NOT EXISTS events (dev TEXT, event TEXT NOT NULL, received UNSIGNED BIGINT NOT NULL, data BLOB, PRIMARY KEY (dev, event));
INSERT INTO events SELECT dev, event, received, data FROM events_backup;

DROP table events_backup;
