-- Indexed file search (owned by the search work stream; range 0030-0039).
--
-- One row per file or folder in every indexed source. `path` is the real path
-- inside the source in VPath display form ("/a/b.txt"); the source root itself
-- is never stored. Times are Unix seconds (UTC).
CREATE TABLE file_index (
    id     INTEGER PRIMARY KEY,
    source TEXT    NOT NULL,
    path   TEXT    NOT NULL,
    name   TEXT    NOT NULL,
    -- Lower-case extension without the dot ('' for none and for folders).
    ext    TEXT    NOT NULL,
    -- Coarse category: dir, image, video, audio, document, text, code, archive, other.
    kind   TEXT    NOT NULL,
    is_dir INTEGER NOT NULL,
    size   INTEGER NOT NULL,
    mtime  INTEGER,
    -- For symlinks: the real path of the target inside the source. Search
    -- only shows a link to users whose scope contains the target too.
    target TEXT,
    -- Scan generation that last saw the row; rows older than a finished scan are swept.
    seen   INTEGER NOT NULL,
    UNIQUE (source, path)
);
CREATE INDEX file_index_kind  ON file_index(source, kind);
CREATE INDEX file_index_size  ON file_index(size);
CREATE INDEX file_index_mtime ON file_index(mtime);

-- Substring search on names. The trigram tokenizer matches any 3+ character
-- fragment, case-insensitively; shorter terms fall back to LIKE.
CREATE VIRTUAL TABLE file_index_fts USING fts5(
    name,
    content = 'file_index',
    content_rowid = 'id',
    tokenize = 'trigram'
);

CREATE TRIGGER file_index_ai AFTER INSERT ON file_index BEGIN
    INSERT INTO file_index_fts(rowid, name) VALUES (new.id, new.name);
END;
CREATE TRIGGER file_index_ad AFTER DELETE ON file_index BEGIN
    INSERT INTO file_index_fts(file_index_fts, rowid, name) VALUES ('delete', old.id, old.name);
END;
-- Only a name change touches the FTS table, so the per-scan `seen` update stays cheap.
CREATE TRIGGER file_index_au AFTER UPDATE OF name ON file_index BEGIN
    INSERT INTO file_index_fts(file_index_fts, rowid, name) VALUES ('delete', old.id, old.name);
    INSERT INTO file_index_fts(rowid, name) VALUES (new.id, new.name);
END;
