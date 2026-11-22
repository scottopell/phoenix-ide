INSERT INTO app_settings(key, value)
SELECT 'slack_gateway_trigger_emoji', name
FROM slack_gateway_trigger_emojis ORDER BY ordinal LIMIT 1;

DELETE FROM slack_gateway_discovery
WHERE (SELECT count(*) FROM slack_gateway_trigger_emojis) > 1;

DROP TABLE slack_gateway_trigger_emojis;
