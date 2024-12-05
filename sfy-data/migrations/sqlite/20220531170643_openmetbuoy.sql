-- Add buoy type
CREATE TEMPORARY TABLE buoys_backup (dev, name);
INSERT INTO buoys_backup SELECT dev, name FROM buoys;

DROP TABLE buoys;

CREATE TABLE IF NOT EXISTS buoys (dev TEXT NOT NULL, name TEXT, buoy_type TEXT NOT NULL, PRIMARY KEY (dev, buoy_type));
INSERT INTO buoys SELECT dev, name, "sfy" FROM buoys_backup;

DROP TABLE buoys_backup;

-- Create events for Floatenstein
CREATE TABLE IF NOT EXISTS omb_events (dev TEXT NOT NULL, account TEXT, event INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, received UNSIGNED BIGINT NOT NULL, message_type TEXT NOT NULL, data BLOB);
