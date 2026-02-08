import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { ConversationListPage } from './pages/ConversationListPage';
import { ConversationPage } from './pages/ConversationPage';
import { NewConversationPage } from './pages/NewConversationPage';
import './index.css';

function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<ConversationListPage />} />
        <Route path="/new" element={<NewConversationPage />} />
        <Route path="/c/:slug" element={<ConversationPage />} />
      </Routes>
    </BrowserRouter>
  );
}

export default App;
