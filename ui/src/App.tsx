import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { ConversationListPage } from './pages/ConversationListPage';
import { ConversationPage } from './pages/ConversationPage';
import { NewConversationPage } from './pages/NewConversationPage';
import { AuthBanner } from './components/AuthBanner';
import { useGlobalKeyboardShortcuts } from './hooks';
import { useAIGatewayAuth } from './hooks/useAIGatewayAuth';
import './index.css';

// Wrapper component to use hooks inside router context
function AppRoutes() {
  useGlobalKeyboardShortcuts();
  
  return (
    <Routes>
      <Route path="/" element={<ConversationListPage />} />
      <Route path="/new" element={<NewConversationPage />} />
      <Route path="/c/:slug" element={<ConversationPage />} />
    </Routes>
  );
}

function App() {
  const { authState, initiateAuth } = useAIGatewayAuth();

  // Only show UI if AI Gateway mode is active
  // (authState will be null or 'not_required' for other LLM providers)
  const needsAuth = authState?.status === 'required';
  const authInProgress =
    authState?.status === 'in_progress' &&
    authState.oauth_url &&
    authState.device_code;

  return (
    <BrowserRouter>
      {/* Only render auth UI if AI Gateway is enabled */}
      {needsAuth && (
        <div className="auth-prompt-banner">
          <span>🔐 AI Gateway authentication required</span>
          <button onClick={initiateAuth} className="auth-prompt-btn">
            Authenticate
          </button>
        </div>
      )}

      {authInProgress && (
        <AuthBanner
          oauthUrl={authState.oauth_url!}
          deviceCode={authState.device_code!}
        />
      )}

      <AppRoutes />
    </BrowserRouter>
  );
}

export default App;
