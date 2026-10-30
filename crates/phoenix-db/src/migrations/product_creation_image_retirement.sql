CREATE TABLE product_creation_image_replay_identity (
    request_id TEXT NOT NULL REFERENCES product_creation_jobs(request_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0 AND ordinal < 20),
    media_type TEXT NOT NULL,
    data_sha256 TEXT NOT NULL CHECK (length(data_sha256) = 64 AND data_sha256 NOT GLOB '*[^0-9A-F]*'),
    PRIMARY KEY (request_id, ordinal)
) WITHOUT ROWID;

CREATE TABLE product_creation_retired_images (
    request_id TEXT PRIMARY KEY REFERENCES product_creation_jobs(request_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TRIGGER product_creation_retire_images_after_product_delete
AFTER DELETE ON product_conversations
BEGIN
    INSERT INTO product_creation_retired_images (request_id)
    SELECT request_id FROM product_creation_jobs WHERE published_product_id = OLD.id;
    DELETE FROM product_creation_job_images
    WHERE request_id IN (SELECT request_id FROM product_creation_jobs WHERE published_product_id = OLD.id);
END;

CREATE TRIGGER product_creation_identity_no_update
BEFORE UPDATE ON product_creation_image_replay_identity
BEGIN SELECT RAISE(ABORT, 'creation replay identity is immutable'); END;

CREATE TRIGGER product_creation_identity_no_delete
BEFORE DELETE ON product_creation_image_replay_identity
WHEN EXISTS (SELECT 1 FROM product_creation_jobs WHERE request_id = OLD.request_id)
BEGIN SELECT RAISE(ABORT, 'creation replay identity is immutable'); END;
