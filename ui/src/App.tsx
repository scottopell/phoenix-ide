import { useState, useEffect } from 'react';
import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { ConversationListPage } from './pages/ConversationListPage';
import { ConversationPage } from './pages/ConversationPage';
import { NewConversationPage } from './pages/NewConversationPage';
import { DesktopLayout } from './components/DesktopLayout';
import { ShortcutHelpPanel } from './components/ShortcutHelpPanel';
import { useGlobalKeyboardShortcuts, FocusScopeProvider } from './hooks';
import { ConversationProvider } from './conversation';
import './index.css';

// Wrapper component to use hooks inside router context
function AppRoutes() {
  useGlobalKeyboardShortcuts();
  const [showHelp, setShowHelp] = useState(false);

  useEffect(() => {
    const handler = () => setShowHelp((prev) => !prev);
    window.addEventListener('toggle-shortcut-help', handler);
    return () => window.removeEventListener('toggle-shortcut-help', handler);
  }, []);

  return (
    <>
      <DesktopLayout>
        <Routes>
          <Route path="/" element={<ConversationListPage />} />
          <Route path="/new" element={<NewConversationPage />} />
          <Route path="/c/:slug" element={<ConversationPage />} />
        </Routes>
      </DesktopLayout>
      <ShortcutHelpPanel visible={showHelp} onClose={() => setShowHelp(false)} />
    </>
  );
}

function App() {
  return (
    <BrowserRouter>
      <FocusScopeProvider>
        <ConversationProvider>
          <AppRoutes />
        </ConversationProvider>
      </FocusScopeProvider>
    </BrowserRouter>
  );
}

export default App;
