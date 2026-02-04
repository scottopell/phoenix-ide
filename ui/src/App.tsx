import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { ConversationListPage } from './pages/ConversationListPage';
import { ConversationPage } from './pages/ConversationPage';
import { PerformanceDashboard } from './components/PerformanceDashboard';
import './index.css';

function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<ConversationListPage />} />
        <Route path="/c/:slug" element={<ConversationPage />} />
      </Routes>
      <PerformanceDashboard />
    </BrowserRouter>
  );
}

export default App;
