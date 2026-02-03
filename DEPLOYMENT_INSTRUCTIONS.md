# Deployment Instructions for Model Registry Update

## Breaking Changes

This update changes model IDs:
- `claude-4-opus` → `claude-4.5-opus`
- `claude-4-sonnet` → `claude-4.5-sonnet`
- `claude-3.5-haiku` → `claude-4.5-haiku`

Existing conversations in the database reference the old model IDs.

## Deployment Options

### Option A: Clean Deployment (Recommended)

This removes all existing conversations and starts fresh.

```bash
# For production deployment
./dev.py prod stop
rm ~/.phoenix-ide/prod.db
./dev.py prod deploy

# For development
./dev.py down
rm ~/.phoenix-ide/phoenix-*.db
./dev.py up
```

### Option B: In-Place Migration

This preserves existing conversations but updates model IDs.

```bash
# Stop the service
./dev.py prod stop  # or ./dev.py down for dev

# Backup database first!
cp ~/.phoenix-ide/prod.db ~/.phoenix-ide/prod.db.backup

# Run migration
sqlite3 ~/.phoenix-ide/prod.db <<EOF
-- Update model IDs in conversations table
UPDATE conversations 
SET model = CASE model
    WHEN 'claude-4-opus' THEN 'claude-4.5-opus'
    WHEN 'claude-4-sonnet' THEN 'claude-4.5-sonnet'
    WHEN 'claude-3.5-haiku' THEN 'claude-4.5-haiku'
    ELSE model
END
WHERE model IN ('claude-4-opus', 'claude-4-sonnet', 'claude-3.5-haiku');

-- Verify the update
SELECT DISTINCT model FROM conversations;
EOF

# Deploy and start
./dev.py prod deploy  # or ./dev.py up for dev
```

## Verification

After deployment:

1. Check the models endpoint:
   ```bash
   curl http://localhost:8000/api/models | jq .
   ```

2. Verify rich metadata is returned:
   - Each model should have id, provider, description, context_window
   - Model IDs should use new format (e.g., `claude-4.5-opus`)

3. Test creating a new conversation in the UI
   - Model dropdown should show descriptions
   - Selected model should save correctly

4. If migrated: Test loading an existing conversation
   - Should load without errors
   - Model should show as updated ID

## Rollback Plan

If issues occur:

```bash
# Stop new version
./dev.py prod stop

# Option A (clean deploy): Just redeploy old version
git checkout <previous-commit>
./dev.py prod deploy

# Option B (migration): Restore backup
mv ~/.phoenix-ide/prod.db.backup ~/.phoenix-ide/prod.db
git checkout <previous-commit>
./dev.py prod deploy
```
