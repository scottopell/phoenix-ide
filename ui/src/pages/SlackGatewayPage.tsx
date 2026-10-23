import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { SlackGatewaySettings } from '../components/SlackGatewaySettings';
import { McpStatusPanel } from '../components/McpStatusPanel';
export function SlackGatewayPage() {
  const navigate = useNavigate();
  const [notice, setNotice] = useState<string | null>(null);
  return <div id="app" className="list-page"><main id="main-area"><section className="view active">
    <div className="view-header"><h2>Slack gateway</h2><button type="button" onClick={() => navigate(-1)}>Back</button></div>
    <div style={{ maxWidth: 800, margin: '0 auto', padding: 16 }}>
      <h3>Slack connection</h3>
      <p>Open MCP below and sign into the existing Slack connection. A conversation is not required.</p>
      <McpStatusPanel showToast={setNotice} showError={setNotice} />
      {notice && <p role="status">{notice}</p>}
      <SlackGatewaySettings standalone />
    </div>
  </section></main></div>;
}
