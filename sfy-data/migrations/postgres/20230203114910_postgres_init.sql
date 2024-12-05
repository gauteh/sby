-- Add migration script here
CREATE TABLE IF NOT EXISTS buoys (dev TEXT NOT NULL, name TEXT, buoy_type TEXT NOT NULL, PRIMARY KEY (dev, buoy_type));

CREATE TABLE events (dev TEXT, event TEXT NOT NULL, received BIGINT NOT NULL, message_type TEXT NOT NULL, data BYTEA, PRIMARY KEY (dev, event));
CREATE TABLE IF NOT EXISTS omb_events (dev TEXT NOT NULL, account TEXT, event SERIAL PRIMARY KEY, received BIGINT NOT NULL, message_type TEXT NOT NULL, data BYTEA);

CREATE INDEX sfy_message_type ON events (dev, received, message_type);
CREATE INDEX omb_message_type ON omb_events (dev, received, message_type);
