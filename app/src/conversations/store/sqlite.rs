use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::conversations::history::RequestUsage;

use super::{
    CATALOGUE_VERSION, ConversationError, ConversationId, ConversationMessage,
    ConversationMetadata, ConversationRecord, MessageFile, MessageId, MessageRole, MessageStatus,
    MetadataFile, TREE_PAGE, TranscriptCursor, TranscriptWindow, TreeEntry, TreeTip, TreeWindow,
    message_from_file, message_to_file, metadata_to_file, model_from_file, parse_stored_network,
    record_from_parts,
};

const DATABASE_NAME: &str = "conversations.sqlite3";

/// One bounded scan for a text search. The page reports a partial result when
/// the scan reaches this budget before the conversation ends.
const TREE_SCAN: usize = 1024;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    title TEXT NOT NULL,
    title_pending INTEGER NOT NULL,
    active_job TEXT,
    continuation INTEGER NOT NULL,
    last_message_status TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    metadata TEXT NOT NULL,
    next_message_sequence INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
    conversation_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    id TEXT NOT NULL,
    message TEXT NOT NULL,
    parent TEXT GENERATED ALWAYS AS (json_extract(message, '$.parent')) STORED,
    PRIMARY KEY (conversation_id, sequence),
    UNIQUE (conversation_id, id),
    FOREIGN KEY (conversation_id) REFERENCES conversations(id)
);
CREATE INDEX IF NOT EXISTS message_parent ON messages(conversation_id, parent, sequence);
CREATE TABLE IF NOT EXISTS compactions (
    conversation_id TEXT NOT NULL,
    covered_through TEXT NOT NULL,
    checkpoint TEXT NOT NULL,
    PRIMARY KEY (conversation_id, covered_through),
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS summary_requests (
    conversation_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    id TEXT NOT NULL,
    request TEXT NOT NULL,
    PRIMARY KEY (conversation_id, sequence),
    UNIQUE (conversation_id, id),
    FOREIGN KEY (conversation_id) REFERENCES conversations(id)
);
";

// Strict append order bounds the walk without cumulative ancestor strings.
// Keep message bodies outside the recursive work table.
const PATH_CTE: &str = "WITH RECURSIVE path(id, parent, sequence, depth) AS (
    SELECT id, parent, sequence, 0 FROM messages
        WHERE conversation_id = ?1 AND id = ?2
    UNION ALL
    SELECT m.id, m.parent, m.sequence, p.depth + 1
        FROM messages m JOIN path p ON m.id = p.parent
        WHERE m.conversation_id = ?1 AND m.sequence < p.sequence
)";

pub(crate) struct Database {
    connection: Connection,
}

impl Database {
    pub(crate) fn open(dir: &Path) -> Result<Self, ConversationError> {
        crate::storage::ensure_private_dir(dir).map_err(|_| ConversationError::Persist)?;
        reject_legacy_records(dir)?;
        let path = dir.join(DATABASE_NAME);
        let connection = Connection::open(&path).map_err(map_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(map_error)?;
        // FULL makes the projection durable before the separate run journal clears.
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;",
            )
            .map_err(map_error)?;
        crate::storage::restrict_private_file(&path).map_err(|_| ConversationError::Persist)?;
        let db = Self { connection };
        db.initialise()?;
        Ok(db)
    }

    fn initialise(&self) -> Result<(), ConversationError> {
        self.connection.execute_batch(SCHEMA).map_err(map_error)
    }

    pub(crate) fn contains(&self, id: &ConversationId) -> Result<bool, ConversationError> {
        self.connection
            .query_row(
                "SELECT 1 FROM conversations WHERE id = ?1",
                [id.as_hex()],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
            .map_err(map_error)
    }

    pub(crate) fn active_ids(&self) -> Result<Vec<ConversationId>, ConversationError> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM conversations WHERE active_job IS NOT NULL")
            .map_err(map_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_error)?;
        let mut ids = Vec::new();
        for row in rows {
            let value = row.map_err(map_error)?;
            ids.push(ConversationId::parse(&value).ok_or(ConversationError::Corrupt)?);
        }
        Ok(ids)
    }

    pub(crate) fn load(
        &self,
        id: &ConversationId,
    ) -> Result<Option<ConversationRecord>, ConversationError> {
        let Some((metadata, summary_requests)) = self.load_header(id)? else {
            return Ok(None);
        };
        // The active path is the only projection that a full load returns.
        // `load_header` fails closed when a stored path has no active leaf.
        let messages = self.load_messages(id, metadata.active_leaf.as_deref())?;
        record_from_parts(metadata, messages, summary_requests, true).map(Some)
    }

    /// Metadata and summary requests without any message body. The caller
    /// pairs this shell with one bounded transcript window.
    pub(crate) fn load_shell(
        &self,
        id: &ConversationId,
    ) -> Result<Option<ConversationRecord>, ConversationError> {
        let Some((metadata, summary_requests)) = self.load_header(id)? else {
            return Ok(None);
        };
        record_from_parts(metadata, Vec::new(), summary_requests, false).map(Some)
    }

    /// Metadata plus the model projection from the committed compaction
    /// boundary to the active leaf. Covered message bodies are not read. The
    /// committed summary itself stays in the metadata, so the caller receives
    /// the summary turn and the retained suffix with one bounded path walk.
    pub(crate) fn load_context(
        &self,
        id: &ConversationId,
    ) -> Result<Option<ConversationRecord>, ConversationError> {
        let Some((metadata, summary_requests)) = self.load_header(id)? else {
            return Ok(None);
        };
        let boundary = metadata
            .compaction
            .as_ref()
            .map(|compaction| compaction.covered_through.clone());
        let files =
            self.load_context_messages(id, metadata.active_leaf.as_deref(), boundary.as_deref())?;
        let messages: Vec<ConversationMessage> = files
            .into_iter()
            .map(message_from_file)
            .collect::<Result<Vec<_>, _>>()?;
        let mut shell = record_from_parts(metadata, Vec::new(), summary_requests, false)?;
        if let Some(compaction) = &shell.compaction
            && !compaction.valid(&messages)
        {
            return Err(ConversationError::Corrupt);
        }
        shell.messages = messages;
        Ok(Some(shell))
    }

    /// Walk the active path and keep entries from one boundary onward. A
    /// missing boundary or leaf reports corruption through the caller.
    fn load_context_messages(
        &self,
        id: &ConversationId,
        leaf: Option<&str>,
        boundary: Option<&str>,
    ) -> Result<Vec<MessageFile>, ConversationError> {
        let Some(leaf) = leaf else {
            return Ok(Vec::new());
        };
        let mut statement = self
            .connection
            .prepare(
                "WITH RECURSIVE path(id, parent, sequence, depth) AS (
                    SELECT id, parent, sequence, 0 FROM messages
                        WHERE conversation_id = ?1 AND id = ?2
                    UNION ALL
                    SELECT m.id, m.parent, m.sequence, p.depth + 1
                        FROM messages m JOIN path p ON m.id = p.parent
                        WHERE m.conversation_id = ?1 AND m.sequence < p.sequence
                            AND (?3 IS NULL OR p.id != ?3)
                ) SELECT id, (SELECT message FROM messages WHERE conversation_id = ?1 AND id = path.id)
                    FROM path ORDER BY depth DESC"
            )
            .map_err(map_error)?;
        let rows = statement
            .query_map(params![id.as_hex(), leaf, boundary], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_error)?;
        let mut messages = Vec::new();
        for row in rows {
            let (key, json) = row.map_err(map_error)?;
            let message: MessageFile =
                serde_json::from_str(&json).map_err(|_| ConversationError::Corrupt)?;
            if message.id != key {
                return Err(ConversationError::Corrupt);
            }
            messages.push(message);
        }
        if messages.last().map(|message| message.id.as_str()) != Some(leaf)
            || messages.first().is_none_or(|message| match boundary {
                Some(boundary) => message.id != boundary,
                None => message.parent.is_some(),
            })
            || messages
                .windows(2)
                .any(|pair| pair[1].parent.as_deref() != Some(pair[0].id.as_str()))
        {
            return Err(ConversationError::Corrupt);
        }
        Ok(messages)
    }

    fn load_header(
        &self,
        id: &ConversationId,
    ) -> Result<Option<(MetadataFile, Vec<super::super::history::RequestUsage>)>, ConversationError>
    {
        let metadata_json: Option<String> = self
            .connection
            .query_row(
                "SELECT metadata FROM conversations WHERE id = ?1",
                [id.as_hex()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_error)?;
        let Some(metadata_json) = metadata_json else {
            return Ok(None);
        };
        let metadata: MetadataFile =
            serde_json::from_str(&metadata_json).map_err(|_| ConversationError::Corrupt)?;
        if metadata.id != id.as_hex() {
            return Err(ConversationError::Corrupt);
        }
        // An active leaf is the only entry point to a retained path. A format
        // without one, but with retained rows, is incompatible and fails closed.
        if metadata.active_leaf.is_none() {
            let has_messages: bool = self
                .connection
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM messages WHERE conversation_id = ?1)",
                    [id.as_hex()],
                    |row| row.get(0),
                )
                .map_err(map_error)?;
            if has_messages {
                return Err(ConversationError::Corrupt);
            }
        }
        let invalid: bool = self
            .connection
            .query_row(
                "SELECT EXISTS (
                SELECT 1 FROM messages m WHERE m.conversation_id = ?1 AND (
                    (m.parent IS NOT NULL AND NOT EXISTS (
                        SELECT 1 FROM messages p WHERE p.conversation_id = m.conversation_id
                        AND p.id = m.parent AND p.sequence < m.sequence
                    ))
                )
            ) OR (?2 IS NOT NULL AND NOT EXISTS (
                SELECT 1 FROM messages WHERE conversation_id = ?1 AND id = ?2
            ))",
                params![id.as_hex(), metadata.active_leaf],
                |row| row.get(0),
            )
            .map_err(map_error)?;
        if invalid {
            return Err(ConversationError::Corrupt);
        }
        let summary_requests = self.load_summary_requests(id)?;
        Ok(Some((metadata, summary_requests)))
    }

    /// Select one bounded window by immutable append order. The window sorts
    /// its rows into presentation order and reports append-order neighbours.
    pub(crate) fn transcript_window(
        &self,
        id: &ConversationId,
        cursor: Option<TranscriptCursor>,
        leaf: Option<MessageId>,
        size: usize,
    ) -> Result<TranscriptWindow, ConversationError> {
        let hex = id.as_hex();
        let active_leaf = self
            .load_header(id)?
            .and_then(|(metadata, _)| metadata.active_leaf)
            .map(|value| MessageId::parse(&value).ok_or(ConversationError::Corrupt))
            .transpose()?;
        let live = cursor.is_none() && leaf.is_none();
        let leaf = leaf.or(active_leaf);
        let size = size.max(1);
        let limit = (size + 1) as i64;
        let Some(leaf) = leaf else {
            if cursor.is_some() {
                return Err(ConversationError::Entry);
            }
            return Ok(TranscriptWindow {
                messages: Vec::new(),
                has_before: false,
                has_after: false,
                before_anchor: None,
                after_anchor: None,
                total: 0,
                live,
            });
        };
        let leaf_hex = leaf.as_hex();
        let exists: bool = self
            .connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM messages WHERE conversation_id = ?1 AND id = ?2)",
                params![hex, leaf_hex],
                |row| row.get(0),
            )
            .map_err(map_error)?;
        if !exists {
            return Err(ConversationError::Entry);
        }
        let total = self.path_count(&hex, &leaf_hex)?;
        let mut has_before = false;
        let mut has_after = false;
        let mut fallback: Option<MessageId> = None;
        // `depth` is the distance from the selected leaf. The live tail and a
        // leaf-ward window select the smallest depths; an earlier window
        // selects the largest depths.
        let rows: Vec<(i64, ConversationMessage)> = match cursor {
            None => {
                let mut rows = self.path_rows(
                    &format!(
                        "{PATH_CTE} SELECT (SELECT message FROM messages WHERE conversation_id = ?1 AND id = path.id), depth FROM path ORDER BY depth ASC LIMIT ?3"
                    ),
                    params![hex, leaf_hex, limit],
                )?;
                if rows.len() > size {
                    has_before = true;
                    rows.truncate(size);
                }
                rows.reverse();
                rows
            }
            Some(cursor) => {
                let message = cursor.message();
                let depth = self
                    .path_depth(&hex, &leaf_hex, &message.as_hex())?
                    .ok_or(ConversationError::Entry)?;
                fallback = Some(message);
                match cursor {
                    TranscriptCursor::Before(_) => {
                        let mut rows = self.path_rows(
                            &format!(
                                "{PATH_CTE} SELECT (SELECT message FROM messages WHERE conversation_id = ?1 AND id = path.id), depth FROM path WHERE depth > ?3 ORDER BY depth ASC LIMIT ?4"
                            ),
                            params![hex, leaf_hex, depth, limit],
                        )?;
                        if rows.len() > size {
                            has_before = true;
                            rows.truncate(size);
                        }
                        has_after = true;
                        rows.reverse();
                        rows
                    }
                    TranscriptCursor::After(_) => {
                        let mut rows = self.path_rows(
                            &format!(
                                "{PATH_CTE} SELECT (SELECT message FROM messages WHERE conversation_id = ?1 AND id = path.id), depth FROM path WHERE depth < ?3 ORDER BY depth DESC LIMIT ?4"
                            ),
                            params![hex, leaf_hex, depth, limit],
                        )?;
                        if rows.len() > size {
                            has_after = true;
                            rows.truncate(size);
                        }
                        has_before = true;
                        rows
                    }
                    TranscriptCursor::Around(_) => {
                        // A canonical entry view keeps byte-budget omissions reachable.
                        let rows = self.path_rows(
                            &format!(
                                "{PATH_CTE} SELECT (SELECT message FROM messages WHERE conversation_id = ?1 AND id = path.id), depth FROM path WHERE depth = ?3 LIMIT 1"
                            ),
                            params![hex, leaf_hex, depth],
                        )?;
                        has_before = depth + 1 < total;
                        has_after = depth > 0;
                        rows
                    }
                }
            }
        };
        let before_anchor = rows
            .iter()
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, message)| message.id)
            .or(fallback);
        let after_anchor = rows
            .iter()
            .min_by_key(|(depth, _)| *depth)
            .map(|(_, message)| message.id)
            .or(fallback);
        Ok(TranscriptWindow {
            messages: rows.into_iter().map(|(_, message)| message).collect(),
            has_before,
            has_after,
            before_anchor,
            after_anchor,
            total: usize::try_from(total).unwrap_or(usize::MAX),
            live,
        })
    }

    fn path_count(&self, conversation: &str, leaf: &str) -> Result<i64, ConversationError> {
        self.connection
            .query_row(
                &format!("{PATH_CTE} SELECT COUNT(*) FROM path"),
                params![conversation, leaf],
                |row| row.get(0),
            )
            .map_err(map_error)
    }

    fn path_depth(
        &self,
        conversation: &str,
        leaf: &str,
        target: &str,
    ) -> Result<Option<i64>, ConversationError> {
        self.connection
            .query_row(
                &format!("{PATH_CTE} SELECT depth FROM path WHERE id = ?3 LIMIT 1"),
                params![conversation, leaf, target],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_error)
    }

    fn path_rows(
        &self,
        sql: &str,
        parameters: impl rusqlite::Params,
    ) -> Result<Vec<(i64, ConversationMessage)>, ConversationError> {
        let mut statement = self.connection.prepare(sql).map_err(map_error)?;
        let rows = statement
            .query_map(parameters, |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(map_error)?;
        let mut result = Vec::new();
        for row in rows {
            let (json, depth) = row.map_err(map_error)?;
            let file: MessageFile =
                serde_json::from_str(&json).map_err(|_| ConversationError::Corrupt)?;
            result.push((depth, super::message_from_file(file)?));
        }
        Ok(result)
    }

    /// One bounded page of retained entries in immutable append order. A
    /// search scans at most `TREE_SCAN` entries and reports a partial result
    /// when that budget ends before the conversation does.
    pub(crate) fn tree_window(
        &self,
        id: &ConversationId,
        after: Option<i64>,
        search: Option<&str>,
        parent: Option<MessageId>,
    ) -> Result<TreeWindow, ConversationError> {
        let hex = id.as_hex();
        let active_leaf = self.metadata(id)?.and_then(|metadata| metadata.active_leaf);
        let parent = parent.map(|id| id.as_hex());
        if let Some(parent) = &parent {
            let exists: bool = self
                .connection
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM messages WHERE conversation_id = ?1 AND id = ?2)",
                    params![hex, parent],
                    |row| row.get(0),
                )
                .map_err(map_error)?;
            if !exists {
                return Err(ConversationError::Entry);
            }
        }
        let total: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1 AND (?2 IS NULL OR parent = ?2)",
                params![hex, parent],
                |row| row.get(0),
            )
            .map_err(map_error)?;
        let after = after.unwrap_or(-1);
        let (entries, has_more, next_cursor, partial) =
            match search.map(str::trim).filter(|query| !query.is_empty()) {
                Some(query) => {
                    let scanned =
                        self.tree_rows(&hex, after, TREE_SCAN, parent.as_deref(), active_leaf)?;
                    let needle = query.to_lowercase();
                    let mut matched: Vec<TreeEntry> = scanned
                        .iter()
                        .filter(|entry| entry.text.to_lowercase().contains(&needle))
                        .cloned()
                        .collect();
                    let full_scan = scanned.len() == TREE_SCAN;
                    let more_rows = full_scan
                        && self.has_rows_after(
                            &hex,
                            scanned.last().map(|e| e.sequence),
                            parent.as_deref(),
                        )?;
                    let truncated = matched.len() > TREE_PAGE;
                    let partial = truncated || more_rows;
                    matched.truncate(TREE_PAGE);
                    let next_cursor = if truncated {
                        matched.last().map(|entry| entry.sequence)
                    } else if more_rows {
                        scanned.last().map(|entry| entry.sequence)
                    } else {
                        None
                    };
                    (matched, partial, next_cursor, partial)
                }
                None => {
                    let mut rows =
                        self.tree_rows(&hex, after, TREE_PAGE + 1, parent.as_deref(), active_leaf)?;
                    let has_more = rows.len() > TREE_PAGE;
                    if has_more {
                        rows.truncate(TREE_PAGE);
                    }
                    let next_cursor = has_more
                        .then(|| rows.last().map(|entry| entry.sequence))
                        .flatten();
                    (rows, has_more, next_cursor, false)
                }
            };
        Ok(TreeWindow {
            entries,
            active_leaf,
            tips: self.tree_tips(&hex)?,
            has_more,
            next_cursor,
            total: usize::try_from(total).unwrap_or(usize::MAX),
            partial,
        })
    }

    // Unlisted tips stay reachable through entry and child navigation.
    fn tree_tips(&self, conversation: &str) -> Result<Vec<TreeTip>, ConversationError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT m.id, COALESCE(json_extract(m.message, '$.text'), '') FROM messages m
                 WHERE m.conversation_id = ?1 AND NOT EXISTS (
                     SELECT 1 FROM messages c
                     WHERE c.conversation_id = m.conversation_id AND c.parent = m.id
                 )
                 ORDER BY m.sequence ASC LIMIT ?2",
            )
            .map_err(map_error)?;
        let rows = statement
            .query_map(params![conversation, TREE_PAGE as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_error)?;
        let mut tips = Vec::new();
        for row in rows {
            let (id, text) = row.map_err(map_error)?;
            tips.push(TreeTip {
                id: MessageId::parse(&id).ok_or(ConversationError::Corrupt)?,
                text,
            });
        }
        Ok(tips)
    }

    fn has_rows_after(
        &self,
        conversation: &str,
        sequence: Option<i64>,
        parent: Option<&str>,
    ) -> Result<bool, ConversationError> {
        let Some(sequence) = sequence else {
            return Ok(false);
        };
        self.connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM messages WHERE conversation_id = ?1 AND sequence > ?2 AND (?3 IS NULL OR parent = ?3))",
                params![conversation, sequence, parent],
                |row| row.get(0),
            )
            .map_err(map_error)
    }

    fn tree_rows(
        &self,
        conversation: &str,
        after: i64,
        limit: usize,
        parent: Option<&str>,
        active_leaf: Option<MessageId>,
    ) -> Result<Vec<TreeEntry>, ConversationError> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "{PATH_CTE} SELECT m.id, m.parent, json_extract(m.message, '$.role'),
                        json_extract(m.message, '$.status'), json_extract(m.message, '$.text'), m.sequence,
                        p.id IS NOT NULL, COALESCE(json_extract(m.message, '$.final-phase'), 0)
                 FROM messages m LEFT JOIN path p ON p.id = m.id
                 WHERE m.conversation_id = ?1 AND m.sequence > ?3
                 AND (?5 IS NULL OR m.parent = ?5)
                 ORDER BY m.sequence ASC LIMIT ?4"
            ))
            .map_err(map_error)?;
        let rows = statement
            .query_map(
                params![
                    conversation,
                    active_leaf.map(|id| id.as_hex()),
                    after,
                    limit as i64,
                    parent
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, bool>(6)?,
                        row.get::<_, bool>(7)?,
                    ))
                },
            )
            .map_err(map_error)?;
        let mut entries = Vec::new();
        for row in rows {
            let (id, parent, role, status, text, sequence, on_active_path, final_phase) =
                row.map_err(map_error)?;
            let parent = match parent.as_deref() {
                Some(value) => Some(MessageId::parse(value).ok_or(ConversationError::Corrupt)?),
                None => None,
            };
            entries.push(TreeEntry {
                id: MessageId::parse(&id).ok_or(ConversationError::Corrupt)?,
                parent,
                role: parse_role(&role)?,
                status: parse_status(&status)?,
                text,
                sequence,
                on_active_path,
                final_phase,
            });
        }
        Ok(entries)
    }

    pub(super) fn load_messages(
        &self,
        id: &ConversationId,
        leaf: Option<&str>,
    ) -> Result<Vec<MessageFile>, ConversationError> {
        let Some(leaf) = leaf else {
            return Ok(Vec::new());
        };
        let mut statement = self
            .connection
            .prepare(&format!(
                "{PATH_CTE} SELECT id, (SELECT message FROM messages WHERE conversation_id = ?1 AND id = path.id) FROM path ORDER BY depth DESC"
            ))
            .map_err(map_error)?;
        let rows = statement
            .query_map(params![id.as_hex(), leaf], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_error)?;
        let mut messages = Vec::new();
        for row in rows {
            let (key, json) = row.map_err(map_error)?;
            let message: MessageFile =
                serde_json::from_str(&json).map_err(|_| ConversationError::Corrupt)?;
            if message.id != key {
                return Err(ConversationError::Corrupt);
            }
            messages.push(message);
        }
        Ok(messages)
    }

    fn load_summary_requests(
        &self,
        id: &ConversationId,
    ) -> Result<Vec<RequestUsage>, ConversationError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, request FROM summary_requests WHERE conversation_id = ?1 ORDER BY sequence ASC",
            )
            .map_err(map_error)?;
        let rows = statement
            .query_map([id.as_hex()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_error)?;
        let mut requests = Vec::new();
        for row in rows {
            let (key, json) = row.map_err(map_error)?;
            let request: RequestUsage =
                serde_json::from_str(&json).map_err(|_| ConversationError::Corrupt)?;
            if request.id.as_hex() != key {
                return Err(ConversationError::Corrupt);
            }
            requests.push(request);
        }
        Ok(requests)
    }

    pub(crate) fn directory_approved(
        &self,
        id: &ConversationId,
        settings: &crate::execution::ExecutionSettings,
        grant: &crate::execution::DirectoryGrant,
    ) -> Result<bool, ConversationError> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT metadata FROM conversations WHERE id = ?1",
                [id.as_hex()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_error)?;
        let Some(json) = json else {
            return Ok(false);
        };
        let metadata: MetadataFile =
            serde_json::from_str(&json).map_err(|_| ConversationError::Corrupt)?;
        if metadata.version != CATALOGUE_VERSION
            || metadata.id != id.as_hex()
            || metadata.directory_approvals.len() > crate::execution::MAXIMUM_DIRECTORY_GRANTS
        {
            return Err(ConversationError::Corrupt);
        }
        Ok(
            super::directory_approvals_from_file(metadata.directory_approvals)?
                .iter()
                .any(|approval| approval.matches(settings, grant)),
        )
    }

    pub(crate) fn metadata(
        &self,
        id: &ConversationId,
    ) -> Result<Option<ConversationMetadata>, ConversationError> {
        let row = self
            .connection
            .query_row(
                "SELECT id, metadata, last_message_status FROM conversations WHERE id = ?1",
                [id.as_hex()],
                metadata_row,
            )
            .optional()
            .map_err(map_error)?;
        row.map(metadata_from_row).transpose()
    }

    pub(crate) fn metadata_all(&self) -> Result<Vec<ConversationMetadata>, ConversationError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, metadata, last_message_status FROM conversations ORDER BY updated_at_ms DESC, id ASC",
            )
            .map_err(map_error)?;
        let rows = statement.query_map([], metadata_row).map_err(map_error)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(metadata_from_row(row.map_err(map_error)?)?);
        }
        Ok(result)
    }

    pub(crate) fn save_automatic_title(
        &mut self,
        id: &ConversationId,
        revision: u32,
        title: &str,
    ) -> Result<(), ConversationError> {
        let changed = self
            .connection
            .execute(
                "UPDATE conversations SET title = ?3, metadata = json_set(metadata, '$.title', ?3)
             WHERE id = ?1 AND revision = ?2",
                params![id.as_hex(), revision, title],
            )
            .map_err(map_error)?;
        if changed == 0 {
            return Err(if self.contains(id)? {
                ConversationError::Conflict
            } else {
                ConversationError::Missing
            });
        }
        Ok(())
    }

    pub(crate) fn record_summary_request(
        &mut self,
        id: &ConversationId,
        job: crate::sessions::JobId,
        request: &RequestUsage,
    ) -> Result<(), ConversationError> {
        let transaction = self.connection.transaction().map_err(map_error)?;
        let hex = id.as_hex();
        let active: Option<Option<String>> = transaction
            .query_row(
                "SELECT active_job FROM conversations WHERE id = ?1",
                [&hex],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_error)?;
        if active.ok_or(ConversationError::Missing)? != Some(job.as_hex()) || !request.valid() {
            return Err(ConversationError::Conflict);
        }
        transaction.execute(
            "INSERT INTO summary_requests (conversation_id, sequence, id, request)
             SELECT ?1, COALESCE(MAX(sequence) + 1, 0), ?2, ?3 FROM summary_requests WHERE conversation_id = ?1
             ON CONFLICT(conversation_id, id) DO UPDATE SET request = excluded.request",
            params![hex, request.id.as_hex(), serde_json::to_string(request).map_err(|_| ConversationError::Persist)?],
        ).map_err(map_error)?;
        transaction.execute(
            "UPDATE conversations SET updated_at_ms = MAX(updated_at_ms, ?2),
             metadata = json_set(metadata, '$.updated-at-ms', MAX(updated_at_ms, ?2)) WHERE id = ?1",
            params![hex, super::now_ms() as i64],
        ).map_err(map_error)?;
        transaction.commit().map_err(map_error)
    }

    pub(crate) fn checkpoint_output(
        &mut self,
        id: &ConversationId,
        job: crate::sessions::JobId,
        reply: crate::providers::AssistantReply,
    ) -> Result<(), ConversationError> {
        let transaction = self.connection.transaction().map_err(map_error)?;
        let hex = id.as_hex();
        let metadata: Option<String> = transaction
            .query_row(
                "SELECT metadata FROM conversations WHERE id = ?1",
                [&hex],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_error)?;
        let mut metadata: MetadataFile =
            serde_json::from_str(&metadata.ok_or(ConversationError::Missing)?)
                .map_err(|_| ConversationError::Corrupt)?;
        if metadata.version != CATALOGUE_VERSION || metadata.id != hex {
            return Err(ConversationError::Corrupt);
        }
        if metadata.active_job.as_deref() != Some(job.as_hex().as_str()) {
            return Err(ConversationError::Conflict);
        }
        let leaf = metadata
            .active_leaf
            .as_deref()
            .ok_or(ConversationError::Corrupt)?;
        let json: String = transaction
            .query_row(
                "SELECT message FROM messages WHERE conversation_id = ?1 AND id = ?2",
                params![hex, leaf],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_error)?
            .ok_or(ConversationError::Corrupt)?;
        let mut message = super::message_from_file(
            serde_json::from_str(&json).map_err(|_| ConversationError::Corrupt)?,
        )?;
        if message.request != Some(job)
            || message.status != MessageStatus::Pending
            || message.role != super::MessageRole::Assistant
        {
            return Err(ConversationError::Conflict);
        }
        super::apply_owned_reply(&mut message, reply);
        super::super::history::validate_exchange(std::slice::from_ref(&message))
            .map_err(|_| ConversationError::Message)?;
        let json = message_json(&message)?;
        // Identity checks stay in SQLite so a checkpoint never loads old response bodies.
        let duplicate: bool = transaction.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM messages m, json_each(m.message, '$.requests') old,
                    json_each(?3, '$.requests') new
                WHERE m.conversation_id = ?1 AND m.id != ?2
                    AND json_extract(old.value, '$.id') = json_extract(new.value, '$.id')
                UNION ALL
                SELECT 1 FROM messages m, json_each(m.message, '$.activity') old,
                    json_each(?3, '$.activity') new
                WHERE m.conversation_id = ?1 AND m.id != ?2
                    AND json_extract(old.value, '$.tool-call.id') = json_extract(new.value, '$.tool-call.id')
            )", params![hex, message.id.as_hex(), json], |row| row.get(0),
        ).map_err(map_error)?;
        if duplicate {
            return Err(ConversationError::Message);
        }
        metadata.updated_at_ms = super::now_ms().max(metadata.updated_at_ms);
        transaction
            .execute(
                "UPDATE messages SET message = ?3 WHERE conversation_id = ?1 AND id = ?2",
                params![hex, message.id.as_hex(), json],
            )
            .map_err(map_error)?;
        transaction
            .execute(
                "UPDATE conversations SET metadata = ?2, updated_at_ms = ?3 WHERE id = ?1",
                params![
                    hex,
                    serde_json::to_string(&metadata).map_err(|_| ConversationError::Persist)?,
                    metadata.updated_at_ms as i64
                ],
            )
            .map_err(map_error)?;
        transaction.commit().map_err(map_error)
    }

    pub(super) fn path_compaction(
        &self,
        id: &ConversationId,
        messages: &[ConversationMessage],
    ) -> Result<Option<super::super::compaction::CompactionRecord>, ConversationError> {
        let Some(leaf) = messages.last() else {
            return Ok(None);
        };
        let checkpoint: Option<String> = self
            .connection
            .query_row(
                &format!(
                    "{PATH_CTE} SELECT c.checkpoint FROM compactions c
                JOIN path p ON p.id = c.covered_through
                WHERE c.conversation_id = ?1 ORDER BY p.depth ASC LIMIT 1"
                ),
                params![id.as_hex(), leaf.id.as_hex()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_error)?;
        checkpoint
            .map(|json| {
                let file = serde_json::from_str(&json).map_err(|_| ConversationError::Corrupt)?;
                super::compaction_from_file(file, messages, true)
            })
            .transpose()
    }

    // Only metadata changes here. Off-path descendants retain their identities.
    pub(crate) fn select_active_leaf(
        &mut self,
        id: &ConversationId,
        expected_revision: u32,
        expected_active_leaf: Option<MessageId>,
        destination: MessageId,
        compaction: Option<&super::super::compaction::CompactionRecord>,
        last_status: MessageStatus,
    ) -> Result<(), ConversationError> {
        let transaction = self.connection.transaction().map_err(map_error)?;
        let hex = id.as_hex();
        let metadata_json: Option<String> = transaction
            .query_row(
                "SELECT metadata FROM conversations WHERE id = ?1",
                [&hex],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_error)?;
        let mut metadata: MetadataFile =
            serde_json::from_str(&metadata_json.ok_or(ConversationError::Missing)?)
                .map_err(|_| ConversationError::Corrupt)?;
        if metadata.version != CATALOGUE_VERSION || metadata.id != hex {
            return Err(ConversationError::Corrupt);
        }
        if metadata.revision != expected_revision {
            return Err(ConversationError::Conflict);
        }
        let current_leaf = match metadata.active_leaf.as_deref() {
            Some(value) => Some(MessageId::parse(value).ok_or(ConversationError::Corrupt)?),
            None => None,
        };
        if current_leaf != expected_active_leaf {
            return Err(ConversationError::Conflict);
        }
        let destination_hex = destination.as_hex();
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM messages WHERE conversation_id = ?1 AND id = ?2)",
                params![hex, destination_hex],
                |row| row.get(0),
            )
            .map_err(map_error)?;
        if !exists {
            return Err(ConversationError::Entry);
        }
        metadata.active_leaf = Some(destination_hex);
        metadata.compaction = compaction.map(super::compaction_to_file);
        metadata.revision = metadata
            .revision
            .checked_add(1)
            .ok_or(ConversationError::Revision)?;
        metadata.updated_at_ms = super::now_ms().max(metadata.updated_at_ms);
        transaction
            .execute(
                "UPDATE conversations SET revision = ?2, metadata = ?3, last_message_status = ?4, updated_at_ms = ?5 WHERE id = ?1",
                params![
                    hex,
                    i64::from(metadata.revision),
                    serde_json::to_string(&metadata).map_err(|_| ConversationError::Persist)?,
                    serde_json::to_string(&last_status).map_err(|_| ConversationError::Persist)?,
                    i64::try_from(metadata.updated_at_ms).unwrap_or(i64::MAX),
                ],
            )
            .map_err(map_error)?;
        transaction.commit().map_err(map_error)
    }

    pub(crate) fn save(
        &mut self,
        previous: Option<&ConversationRecord>,
        record: &ConversationRecord,
    ) -> Result<(), ConversationError> {
        let transaction = self.connection.transaction().map_err(map_error)?;
        write_conversation(&transaction, previous, record)?;
        transaction.commit().map_err(map_error)
    }

    /// Commit two conversation projections in one transaction. Ownership
    /// transfer needs both rows before the run journal can be cleared.
    pub(crate) fn save_pair(
        &mut self,
        first: Option<&ConversationRecord>,
        first_record: &ConversationRecord,
        second: Option<&ConversationRecord>,
        second_record: &ConversationRecord,
    ) -> Result<(), ConversationError> {
        let transaction = self.connection.transaction().map_err(map_error)?;
        write_conversation(&transaction, first, first_record)?;
        write_conversation(&transaction, second, second_record)?;
        transaction.commit().map_err(map_error)
    }

    pub(crate) fn remove(&mut self, id: &ConversationId) -> Result<(), ConversationError> {
        let transaction = self.connection.transaction().map_err(map_error)?;
        let hex = id.as_hex();
        transaction
            .execute("DELETE FROM messages WHERE conversation_id = ?1", [&hex])
            .map_err(map_error)?;
        transaction
            .execute(
                "DELETE FROM summary_requests WHERE conversation_id = ?1",
                [&hex],
            )
            .map_err(map_error)?;
        transaction
            .execute("DELETE FROM conversations WHERE id = ?1", [&hex])
            .map_err(map_error)?;
        transaction.commit().map_err(map_error)
    }
}

type MetadataRow = (String, String, Option<String>);

fn metadata_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MetadataRow> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

fn metadata_from_row(row: MetadataRow) -> Result<ConversationMetadata, ConversationError> {
    let (key, metadata_json, last_status) = row;
    let metadata: MetadataFile =
        serde_json::from_str(&metadata_json).map_err(|_| ConversationError::Corrupt)?;
    if metadata.version != CATALOGUE_VERSION
        || metadata.id != key
        || metadata.revision == 0
        || metadata.updated_at_ms < metadata.created_at_ms
        || super::normalise_title(&metadata.title).as_ref() != Ok(&metadata.title)
    {
        return Err(ConversationError::Corrupt);
    }
    let id = ConversationId::parse(&metadata.id).ok_or(ConversationError::Corrupt)?;
    let network = parse_stored_network(&metadata.network, &metadata.network_domains)?;
    let mut model = metadata.model.map(model_from_file).transpose()?;
    if let Some(model) = &mut model {
        model.settings.network = network.clone();
    }
    let active_job = match metadata.active_job {
        Some(value) => {
            Some(crate::sessions::JobId::parse(&value).ok_or(ConversationError::Corrupt)?)
        }
        None => None,
    };
    let last_message_status = match last_status {
        Some(value) => Some(
            serde_json::from_str::<MessageStatus>(&value)
                .map_err(|_| ConversationError::Corrupt)?,
        ),
        None => None,
    };
    let active_leaf = match metadata.active_leaf.as_deref() {
        Some(value) => Some(MessageId::parse(value).ok_or(ConversationError::Corrupt)?),
        None => None,
    };
    Ok(ConversationMetadata {
        id,
        revision: metadata.revision,
        title: metadata.title,
        title_pending: metadata.title_pending,
        network,
        model,
        active_job,
        continuation: metadata.continuation.is_some(),
        active_leaf,
        created_at_ms: metadata.created_at_ms,
        updated_at_ms: metadata.updated_at_ms,
        last_message_status,
    })
}

fn write_conversation(
    transaction: &Transaction<'_>,
    previous: Option<&ConversationRecord>,
    record: &ConversationRecord,
) -> Result<(), ConversationError> {
    let metadata = metadata_to_file(record);
    let metadata_json = serde_json::to_string(&metadata).map_err(|_| ConversationError::Persist)?;
    let last_status = record
        .messages
        .last()
        .map(|message| serde_json::to_string(&message.status))
        .transpose()
        .map_err(|_| ConversationError::Persist)?;
    let hex = record.id.as_hex();
    transaction
        .execute(
            "INSERT INTO conversations
                (id, version, revision, title, title_pending, active_job, continuation, last_message_status, created_at_ms, updated_at_ms, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                version = excluded.version,
                revision = excluded.revision,
                title = excluded.title,
                title_pending = excluded.title_pending,
                active_job = excluded.active_job,
                continuation = excluded.continuation,
                last_message_status = excluded.last_message_status,
                created_at_ms = excluded.created_at_ms,
                updated_at_ms = excluded.updated_at_ms,
                metadata = excluded.metadata",
            params![
                hex,
                i64::from(CATALOGUE_VERSION),
                i64::from(record.revision),
                record.title,
                i64::from(record.title_pending),
                record.active_job.map(|job| job.as_hex()),
                i64::from(record.continuation.is_some()),
                last_status,
                i64::try_from(record.created_at_ms).unwrap_or(i64::MAX),
                i64::try_from(record.updated_at_ms).unwrap_or(i64::MAX),
                metadata_json,
            ],
        )
        .map_err(map_error)?;

    write_messages(transaction, &hex, previous, record)?;
    write_summary_requests(transaction, &hex, previous, record)?;
    if previous.and_then(|record| record.compaction.as_ref()) != record.compaction.as_ref()
        && let Some(checkpoint) = &record.compaction
    {
        transaction
            .execute(
                "INSERT INTO compactions (conversation_id, covered_through, checkpoint)
             VALUES (?1, ?2, ?3) ON CONFLICT(conversation_id, covered_through)
             DO UPDATE SET checkpoint = excluded.checkpoint",
                params![
                    hex,
                    checkpoint.covered_through.as_hex(),
                    serde_json::to_string(&super::compaction_to_file(checkpoint))
                        .map_err(|_| ConversationError::Persist)?
                ],
            )
            .map_err(map_error)?;
    }
    Ok(())
}

fn write_messages(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    previous: Option<&ConversationRecord>,
    record: &ConversationRecord,
) -> Result<(), ConversationError> {
    let previous: std::collections::BTreeMap<_, _> = previous
        .into_iter()
        .flat_map(|record| &record.messages)
        .map(|message| (message.id, message))
        .collect();
    let mut retained = std::collections::BTreeSet::new();
    let mut parent: Option<super::MessageId> = None;
    for message in &record.messages {
        if message.parent != parent || !retained.insert(message.id) {
            return Err(ConversationError::Message);
        }
        if let Some(old) = previous.get(&message.id) {
            if old.parent != parent || old.response != message.response {
                return Err(ConversationError::Message);
            }
            if *old != message {
                transaction
                    .execute(
                        "UPDATE messages SET message = ?3 WHERE conversation_id = ?1 AND id = ?2",
                        params![conversation_id, message.id.as_hex(), message_json(message)?],
                    )
                    .map_err(map_error)?;
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO messages (conversation_id, sequence, id, message)
                 SELECT id, next_message_sequence, ?2, ?3 FROM conversations WHERE id = ?1",
                    params![conversation_id, message.id.as_hex(), message_json(message)?],
                )
                .map_err(map_error)?;
            transaction.execute(
                "UPDATE conversations SET next_message_sequence = next_message_sequence + 1 WHERE id = ?1",
                [conversation_id],
            ).map_err(map_error)?;
        }
        parent = Some(message.id);
    }
    for id in previous.keys().filter(|id| !retained.contains(id)) {
        transaction
            .execute(
                "DELETE FROM messages WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id, id.as_hex()],
            )
            .map_err(map_error)?;
    }
    Ok(())
}

fn message_json(message: &super::ConversationMessage) -> Result<String, ConversationError> {
    let file: MessageFile = message_to_file(message);
    let json = serde_json::to_string(&file).map_err(|_| ConversationError::Persist)?;
    super::message_from_file(file).map_err(|_| ConversationError::Message)?;
    Ok(json)
}

fn write_summary_requests(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    previous: Option<&ConversationRecord>,
    record: &ConversationRecord,
) -> Result<(), ConversationError> {
    let previous = previous.map_or(&[][..], |record| record.summary_requests.as_slice());
    if previous.len() > record.summary_requests.len()
        || previous
            .iter()
            .zip(&record.summary_requests)
            .any(|(old, new)| old.id != new.id)
    {
        return Err(ConversationError::Conflict);
    }
    for (sequence, request) in record
        .summary_requests
        .iter()
        .enumerate()
        .skip(previous.len())
    {
        transaction
                .execute(
                    "INSERT INTO summary_requests (conversation_id, sequence, id, request) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        conversation_id,
                        i64::try_from(sequence).unwrap_or(i64::MAX),
                        request.id.as_hex(),
                        serde_json::to_string(request).map_err(|_| ConversationError::Persist)?
                    ],
                )
                .map_err(map_error)?;
    }
    for (sequence, (old, new)) in previous.iter().zip(&record.summary_requests).enumerate() {
        if old != new {
            transaction
                .execute(
                    "UPDATE summary_requests SET request = ?3 WHERE conversation_id = ?1 AND sequence = ?2",
                    params![
                        conversation_id,
                        i64::try_from(sequence).unwrap_or(i64::MAX),
                        serde_json::to_string(new).map_err(|_| ConversationError::Persist)?
                    ],
                )
                .map_err(map_error)?;
        }
    }
    Ok(())
}

/// An alpha format that still holds JSON records fails closed. The database is
/// incompatible with those records, and no migration runs during alpha.
fn reject_legacy_records(dir: &Path) -> Result<(), ConversationError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(ConversationError::Persist),
    };
    for entry in entries {
        let entry = entry.map_err(|_| ConversationError::Persist)?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            return Err(ConversationError::Corrupt);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

fn parse_role(value: &str) -> Result<MessageRole, ConversationError> {
    match value {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        _ => Err(ConversationError::Corrupt),
    }
}

fn parse_status(value: &str) -> Result<MessageStatus, ConversationError> {
    match value {
        "complete" => Ok(MessageStatus::Complete),
        "pending" => Ok(MessageStatus::Pending),
        "interrupted" => Ok(MessageStatus::Interrupted),
        "failed" => Ok(MessageStatus::Failed),
        _ => Err(ConversationError::Corrupt),
    }
}

fn map_error(error: rusqlite::Error) -> ConversationError {
    if let rusqlite::Error::SqliteFailure(code, _) = &error {
        return match code.code {
            rusqlite::ErrorCode::DiskFull => ConversationError::Full,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                ConversationError::Busy
            }
            // An I/O failure during commit can leave the outcome unknown. The
            // caller records that conversation as uncertain and blocks new work.
            rusqlite::ErrorCode::SystemIoFailure => ConversationError::Unsettled,
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => {
                ConversationError::Corrupt
            }
            _ => ConversationError::Persist,
        };
    }
    ConversationError::Persist
}
