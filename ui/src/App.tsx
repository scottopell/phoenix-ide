import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { ConversationListPage } from './pages/ConversationListPage';
import { ConversationPage } from './pages/ConversationPage';
import { PerformanceDashboard } from './components/PerformanceDashboard';
import { ServiceWorkerUpdatePrompt } from './components/ServiceWorkerUpdatePrompt';
import { LayoutDebugOverlay } from './components/LayoutDebugOverlay';
import './index.css';

function App() {
  return (
    <BrowserRouter>
      <ServiceWorkerUpdatePrompt />
      <Routes>
        <Route path="/" element={<ConversationListPage />} />
        <Route path="/c/:slug" element={<ConversationPage />} />
      </Routes>
      <PerformanceDashboard />
      <LayoutDebugOverlay />
    </BrowserRouter>
  );
}

export default App;
