-- This connection-local index avoids transcript scans during observation.
-- Triggers keep it consistent with commits and rollbacks. It holds no authority.
CREATE TEMP TABLE request_usage (
    conversation_id TEXT NOT NULL,
    source TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request TEXT NOT NULL,
    PRIMARY KEY (conversation_id, source, request_id)
);
CREATE INDEX request_usage_identity ON request_usage(conversation_id, request_id);

INSERT INTO request_usage
SELECT m.conversation_id, 'message:' || m.id, json_extract(j.value, '$.id'),
    json_remove(j.value, '$.sources', '$.advertised')
FROM messages m, json_each(m.message, '$.requests') j;
INSERT INTO request_usage
SELECT conversation_id, 'summary:' || id, id,
    json_remove(request, '$.sources', '$.advertised')
FROM summary_requests;

CREATE TEMP TRIGGER usage_message_insert AFTER INSERT ON main.messages BEGIN
    INSERT INTO request_usage
    SELECT NEW.conversation_id, 'message:' || NEW.id, json_extract(value, '$.id'),
        json_remove(value, '$.sources', '$.advertised')
    FROM json_each(NEW.message, '$.requests');
END;
CREATE TEMP TRIGGER usage_message_update AFTER UPDATE OF message ON main.messages
WHEN json_extract(OLD.message, '$.requests') IS NOT json_extract(NEW.message, '$.requests') BEGIN
    DELETE FROM request_usage
    WHERE conversation_id = OLD.conversation_id AND source = 'message:' || OLD.id;
    INSERT INTO request_usage
    SELECT NEW.conversation_id, 'message:' || NEW.id, json_extract(value, '$.id'),
        json_remove(value, '$.sources', '$.advertised')
    FROM json_each(NEW.message, '$.requests');
END;
CREATE TEMP TRIGGER usage_message_delete AFTER DELETE ON main.messages BEGIN
    DELETE FROM request_usage
    WHERE conversation_id = OLD.conversation_id AND source = 'message:' || OLD.id;
END;

CREATE TEMP TRIGGER usage_summary_insert AFTER INSERT ON main.summary_requests BEGIN
    INSERT INTO request_usage VALUES (
        NEW.conversation_id, 'summary:' || NEW.id, NEW.id,
        json_remove(NEW.request, '$.sources', '$.advertised')
    );
END;
CREATE TEMP TRIGGER usage_summary_update AFTER UPDATE OF request ON main.summary_requests BEGIN
    UPDATE request_usage SET request = json_remove(NEW.request, '$.sources', '$.advertised')
    WHERE conversation_id = OLD.conversation_id AND source = 'summary:' || OLD.id;
END;
CREATE TEMP TRIGGER usage_summary_delete AFTER DELETE ON main.summary_requests BEGIN
    DELETE FROM request_usage
    WHERE conversation_id = OLD.conversation_id AND source = 'summary:' || OLD.id;
END;
