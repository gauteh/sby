-- Index message type on OMB
CREATE INDEX omb_message_type ON omb_events (dev, received, message_type);

-- Add message type field to SFY
CREATE TEMPORARY TABLE events_backup (dev, event, received, data);
INSERT INTO events_backup SELECT dev, event, received, data FROM events;

DROP TABLE events;

CREATE TABLE events (dev TEXT, event TEXT NOT NULL, received UNSIGNED BIGINT NOT NULL, message_type TEXT NOT NULL, data BLOB, PRIMARY KEY (dev, event));
CREATE INDEX sfy_message_type ON events (dev, received, message_type);

INSERT INTO events SELECT dev, event, received, replace(iif(instr(event, '_'), substr(event, instr(event, '_')+1), 'unknown'), '.json', '') as message_type, data FROM events_backup;


DROP table events_backup;
