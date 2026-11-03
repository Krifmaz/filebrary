-- A share link can point at a collection instead of paths in one source.
-- For such shares `source` is '' and `paths` is '[]'; the items are read from
-- `collection_items` each time the link is opened. Deleting the collection
-- deletes its links.
ALTER TABLE shares ADD COLUMN collection_id INTEGER REFERENCES collections(id) ON DELETE CASCADE;
CREATE INDEX shares_collection ON shares(collection_id);
