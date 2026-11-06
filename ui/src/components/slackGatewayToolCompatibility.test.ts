import { describe, expect, it } from 'vitest';
import { slackGatewayArgumentProbes, slackGatewayToolProblem, type SlackToolSchema } from './slackGatewayToolCompatibility';

function catalog(): SlackToolSchema[] {
  return Object.keys(slackGatewayArgumentProbes).map(name => ({ server: 'slack', name, input_schema: { type: 'object' } }));
}

describe('Slack gateway schema compatibility', () => {
  it('accepts supported schemas and namespace suffixes', () => {
    const tools = catalog().map(tool => ({ ...tool, name: 'connector_' + tool.name }));
    expect(slackGatewayToolProblem(tools, 'slack')).toBeNull();
  });
  it('does not borrow tools from another connection or accept ambiguous names', () => {
    expect(slackGatewayToolProblem(catalog(), 'other')).toContain('Required tools unavailable');
    const tools = catalog(); tools.push({ ...tools[0]! });
    expect(slackGatewayToolProblem(tools, 'slack')).toContain('exactly one');
  });
  it.each([
    { type: 'object', properties: { text: { type: 'string' } }, additionalProperties: false },
    { type: 'object', properties: { message: { type: 'number' } } },
    { type: 'object', required: ['text'], properties: { text: { type: 'string' } } },
    { type: 'object', properties: { reply_broadcast: { const: true } } },
  ])('detects incompatible advertised send arguments: %j', schema => {
    const tools = catalog(); tools.find(tool => tool.name === 'slack_send_message')!.input_schema = schema;
    expect(slackGatewayToolProblem(tools, 'slack')).toContain('Incompatible tool schema: slack_send_message');
  });
  it('isolates schema identifiers between independently advertised tools', () => {
    const tools = catalog().map(tool => ({ ...tool, input_schema: { $id: 'https://example.test/arguments', type: 'object' } }));
    expect(slackGatewayToolProblem(tools, 'slack')).toBeNull();
  });
  it('reports unsupported schemas as unverified instead of healthy', () => {
    const tools = catalog(); tools[0]!.input_schema = { $ref: 'https://unavailable.example/schema' };
    expect(slackGatewayToolProblem(tools, 'slack')).toContain('Cannot verify');
  });
  it('accepts an explicitly advertised draft-07 schema', () => {
    const tools = catalog(); tools[0]!.input_schema = { $schema: 'http://json-schema.org/draft-07/schema#', type: 'object' };
    expect(slackGatewayToolProblem(tools, 'slack')).toBeNull();
  });
});
