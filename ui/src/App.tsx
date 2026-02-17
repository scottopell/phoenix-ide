import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { ConversationListPage } from './pages/ConversationListPage';
import { ConversationPage } from './pages/ConversationPage';
import { NewConversationPage } from './pages/NewConversationPage';
import { useGlobalKeyboardShortcuts } from './hooks';
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
  return (
    <BrowserRouter>
      <AppRoutes />
    </BrowserRouter>
  );
}

export default App;
