import Ajv from 'ajv';
import Ajv2020 from 'ajv/dist/2020';
import type { AnySchema } from 'ajv';

export interface SlackToolSchema { server: string; name: string; input_schema: unknown }
const channel = 'C0123456789';
const timestamp = '1700000000.000001';
const search = { filters: 'after:2026-01-01', keywords: [':fire:'], natural_language_query: '', limit: 1, response_format: 'detailed', include_context: false, include_bots: false, sort: 'timestamp', sort_dir: 'desc' };
// Probe the argument forms the adapter sends; this does not prove live access.
export const slackGatewayArgumentProbes: Record<string, Record<string, unknown>[]> = {
  slack_read_user_profile: [{ response_format: 'detailed' }],
  slack_search_public_and_private: [search, { ...search, cursor: 'next-page' }, { ...search, keywords: [], include_bots: true }, { ...search, keywords: [], include_bots: true, cursor: 'next-page' }],
  slack_read_thread: [
    { channel_id: channel, message_ts: timestamp, limit: 100, response_format: 'detailed' },
    { channel_id: channel, message_ts: timestamp, oldest: '1700000000.000000', latest: '1700000000.000002', limit: 1, response_format: 'detailed' },
  ],
  slack_get_reactions: [{ channel_id: channel, message_ts: timestamp }],
  slack_add_reaction: [{ channel_id: channel, message_ts: timestamp, emoji: 'eyes' }],
  slack_send_message: [{ channel_id: channel, thread_ts: timestamp, message: 'Phoenix: Example answer', reply_broadcast: false, unfurl_app_links: false }],
};

export function slackGatewayToolProblem(tools: SlackToolSchema[], server: string): string | null {
  for (const [name, probes] of Object.entries(slackGatewayArgumentProbes)) {
    const matches = tools.filter(tool => tool.server === server && (tool.name === name || tool.name.endsWith('_' + name)));
    const [tool] = matches;
    if (!tool || matches.length !== 1) return `Required tools unavailable: expected exactly one ${name}.`;
    const schema = tool.input_schema;
    if (typeof schema !== 'boolean' && (!schema || typeof schema !== 'object' || Array.isArray(schema))) return `Cannot verify ${name}: no input schema.`;
    try {
      const version = typeof schema === 'object' && schema !== null ? (schema as { $schema?: string }).$schema : undefined;
      const validator = version?.includes('draft-07') ? new Ajv({ allErrors: true }) : new Ajv2020({ allErrors: true });
      const validate = validator.compile(schema as AnySchema);
      if (probes.some(probe => !validate(probe))) return `Incompatible tool schema: ${name} rejects gateway arguments.`;
    } catch {
      return `Cannot verify ${name}: unsupported or invalid input schema.`;
    }
  }
  return null;
}
