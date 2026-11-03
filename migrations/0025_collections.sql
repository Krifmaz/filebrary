-- Collections: named lists of files and folders, possibly from several
-- sources, that can be shared with one link. Owned by the
-- comments/collections/themes work stream (shares range 0020-0029).
--
-- Items store the *real* path inside the source. Sharing a collection links a
-- row in `shares` to it through `shares.collection_id` (added next to the
-- shares table, see 0026).

CREATE TABLE collections (
    id         INTEGER PRIMARY KEY,
    owner_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX collections_owner ON collections(owner_id);

CREATE TABLE collection_items (
    id            INTEGER PRIMARY KEY,
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    source        TEXT    NOT NULL,
    path          TEXT    NOT NULL,
    added_at      INTEGER NOT NULL,
    UNIQUE (collection_id, source, path)
);
