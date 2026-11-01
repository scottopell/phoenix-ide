import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { ArrowLeft, MessagesSquare } from 'lucide-react';
import { SlackGatewaySettings } from '../components/SlackGatewaySettings';
import { McpStatusPanel } from '../components/McpStatusPanel';
import './SlackGatewayPage.css';

export function SlackGatewayPage() {
  const navigate = useNavigate();
  const [notice, setNotice] = useState<string | null>(null);
  return <div id="app" className="list-page slack-gateway-page"><main id="main-area"><section className="view active">
    <div className="sg-content">
      <header className="sg-page-header">
        <div className="sg-page-title"><span className="sg-page-icon"><MessagesSquare size={24} aria-hidden="true" /></span><div><h1>Slack gateway</h1><p>Bring a Slack discussion into Phoenix. Keep the work on your machine.</p></div></div>
        <button type="button" className="btn-secondary sg-back" onClick={() => navigate(-1)}><ArrowLeft size={15} aria-hidden="true" /> Back</button>
      </header>
      <section className="sg-card" aria-labelledby="sg-connection-title">
        <div className="sg-card-heading"><div><h2 id="sg-connection-title">Slack connection</h2><p>Sign in to your existing Slack MCP connection. No conversation required.</p></div></div>
        <McpStatusPanel showToast={setNotice} showError={setNotice} />
        {notice && <p className="sg-feedback" role="status">{notice}</p>}
      </section>
      <SlackGatewaySettings standalone />
    </div>
  </section></main></div>;
}
