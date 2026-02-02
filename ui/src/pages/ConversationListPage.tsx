import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, Conversation } from '../api';
import { ConversationList } from '../components/ConversationList';
import { NewConversationModal } from '../components/NewConversationModal';

export function ConversationListPage() {
  const navigate = useNavigate();
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [showModal, setShowModal] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    loadConversations();
  }, []);

  const loadConversations = async () => {
    try {
      const convs = await api.listConversations();
      setConversations(convs);
    } catch (err) {
      console.error('Failed to load conversations:', err);
    } finally {
      setLoading(false);
    }
  };

  const handleCreated = (conv: { id: string; slug: string }) => {
    setShowModal(false);
    // Reload the full list to get proper conversation data
    loadConversations();
    navigate(`/c/${conv.slug}`);
  };

  return (
    <div id="app" className="list-page">
      <main id="main-area">
        {loading ? (
          <div className="empty-state">
            <div className="spinner"></div>
            <p>Loading...</p>
          </div>
        ) : (
          <ConversationList
            conversations={conversations}
            onNewConversation={() => setShowModal(true)}
          />
        )}
      </main>
      <NewConversationModal
        visible={showModal}
        onClose={() => setShowModal(false)}
        onCreated={handleCreated}
      />
    </div>
  );
}
