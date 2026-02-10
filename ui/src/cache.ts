// Phoenix IDE Cache Layer using IndexedDB
// Simplified: Pure get/put storage, no TTL or staleness logic

import { generateUUID } from './utils/uuid';
import type { Conversation, Message } from './api';

const DB_NAME = 'phoenix-ide-cache';
const DB_VERSION = 1;

export interface PendingOperationPayload {
  text?: string;
  images?: { data: string; media_type: string }[];
  localId?: string;
  name?: string;
}

export interface PendingOperation {
  id: string;
  type: 'send_message' | 'archive' | 'unarchive' | 'delete' | 'rename';
  conversationId: string;
  payload: PendingOperationPayload;
  createdAt: Date;
  retryCount: number;
  status: 'pending' | 'processing' | 'failed';
}

export class CacheDB {
  private db: IDBDatabase | null = null;
  private initPromise: Promise<void> | null = null;

  async init(): Promise<void> {
    if (this.db) return;
    if (this.initPromise) return this.initPromise;

    this.initPromise = this.openDB();
    await this.initPromise;
  }

  private async openDB(): Promise<void> {
    return new Promise((resolve, reject) => {
      const request = indexedDB.open(DB_NAME, DB_VERSION);

      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        this.db = request.result;
        resolve();
      };

      request.onupgradeneeded = (event) => {
        const db = (event.target as IDBOpenDBRequest).result;

        // Conversations store
        if (!db.objectStoreNames.contains('conversations')) {
          const convStore = db.createObjectStore('conversations', { keyPath: 'id' });
          convStore.createIndex('by-slug', 'slug', { unique: true });
          convStore.createIndex('by-updated', 'updated_at');
        }

        // Messages store
        if (!db.objectStoreNames.contains('messages')) {
          const msgStore = db.createObjectStore('messages', {
            keyPath: ['conversation_id', 'sequence_id']
          });
          msgStore.createIndex('by-conversation', 'conversation_id');
        }

        // Pending operations for offline sync
        if (!db.objectStoreNames.contains('pendingOps')) {
          const opsStore = db.createObjectStore('pendingOps', { keyPath: 'id' });
          opsStore.createIndex('by-created', 'createdAt');
          opsStore.createIndex('by-conversation', 'conversationId');
        }
      };
    });
  }

  // Conversations - simple get/put
  async getConversation(id: string): Promise<Conversation | null> {
    await this.init();
    const tx = this.db!.transaction(['conversations'], 'readonly');
    const store = tx.objectStore('conversations');
    return new Promise((resolve) => {
      const request = store.get(id);
      request.onsuccess = () => resolve(request.result || null);
      request.onerror = () => resolve(null);
    });
  }

  async getConversationBySlug(slug: string): Promise<Conversation | null> {
    await this.init();
    const tx = this.db!.transaction(['conversations'], 'readonly');
    const store = tx.objectStore('conversations');
    const index = store.index('by-slug');
    return new Promise((resolve) => {
      const request = index.get(slug);
      request.onsuccess = () => resolve(request.result || null);
      request.onerror = () => resolve(null);
    });
  }

  async getAllConversations(): Promise<Conversation[]> {
    await this.init();
    const tx = this.db!.transaction(['conversations'], 'readonly');
    const store = tx.objectStore('conversations');
    return new Promise((resolve) => {
      const request = store.getAll();
      request.onsuccess = () => {
        const conversations = request.result || [];
        // Sort by updated_at descending (most recent first)
        conversations.sort((a: Conversation, b: Conversation) => 
          new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime()
        );
        resolve(conversations);
      };
      request.onerror = () => resolve([]);
    });
  }

  async putConversation(conversation: Conversation): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['conversations'], 'readwrite');
    const store = tx.objectStore('conversations');
    store.put(conversation);
  }

  async putConversations(conversations: Conversation[]): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['conversations'], 'readwrite');
    const store = tx.objectStore('conversations');
    for (const conversation of conversations) {
      store.put(conversation);
    }
  }

  /**
   * Replace all conversations in cache with fresh data from server.
   * This removes any stale entries that no longer exist on the server.
   */
  async syncConversations(conversations: Conversation[]): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['conversations'], 'readwrite');
    const store = tx.objectStore('conversations');
    
    // Get IDs of fresh conversations
    const freshIds = new Set(conversations.map(c => c.id));
    
    // Get all existing conversations
    const existingRequest = store.getAll();
    
    await new Promise<void>((resolve, reject) => {
      existingRequest.onsuccess = () => {
        const existing = existingRequest.result || [];
        
        // Delete conversations that are no longer on server
        for (const conv of existing) {
          if (!freshIds.has(conv.id)) {
            store.delete(conv.id);
          }
        }
        
        // Put fresh conversations
        for (const conv of conversations) {
          store.put(conv);
        }
        
        resolve();
      };
      existingRequest.onerror = () => reject(existingRequest.error);
    });
  }

  async deleteConversation(id: string): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['conversations', 'messages'], 'readwrite');
    
    // Delete conversation
    tx.objectStore('conversations').delete(id);
    
    // Delete all messages for this conversation
    const msgStore = tx.objectStore('messages');
    const index = msgStore.index('by-conversation');
    const range = IDBKeyRange.only(id);
    const request = index.openCursor(range);
    
    request.onsuccess = () => {
      const cursor = request.result;
      if (cursor) {
        cursor.delete();
        cursor.continue();
      }
    };
  }

  // Messages - simple get/put
  async getMessages(conversationId: string): Promise<Message[]> {
    await this.init();
    const tx = this.db!.transaction(['messages'], 'readonly');
    const store = tx.objectStore('messages');
    const index = store.index('by-conversation');
    const range = IDBKeyRange.only(conversationId);
    
    return new Promise((resolve) => {
      const request = index.getAll(range);
      
      request.onsuccess = () => {
        const messages = request.result || [];
        // Sort by sequence
        messages.sort((a: Message, b: Message) => a.sequence_id - b.sequence_id);
        resolve(messages);
      };
      
      request.onerror = () => resolve([]);
    });
  }

  async putMessage(message: Message): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['messages'], 'readwrite');
    const store = tx.objectStore('messages');
    store.put(message);
  }

  async putMessages(messages: Message[]): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['messages'], 'readwrite');
    const store = tx.objectStore('messages');
    for (const message of messages) {
      store.put(message);
    }
  }

  // Pending operations for offline support
  async addPendingOp(op: Omit<PendingOperation, 'id'>): Promise<string> {
    await this.init();
    const id = generateUUID();
    const pendingOp: PendingOperation = {
      ...op,
      id,
      createdAt: new Date(),
      retryCount: 0,
      status: 'pending'
    };
    
    const tx = this.db!.transaction(['pendingOps'], 'readwrite');
    const store = tx.objectStore('pendingOps');
    store.add(pendingOp);
    
    return id;
  }

  async getPendingOps(): Promise<PendingOperation[]> {
    await this.init();
    const tx = this.db!.transaction(['pendingOps'], 'readonly');
    const store = tx.objectStore('pendingOps');
    const index = store.index('by-created');
    
    return new Promise((resolve) => {
      const request = index.getAll();
      request.onsuccess = () => resolve(request.result || []);
      request.onerror = () => resolve([]);
    });
  }

  async updatePendingOp(id: string, updates: Partial<PendingOperation>): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['pendingOps'], 'readwrite');
    const store = tx.objectStore('pendingOps');
    
    const existing = await new Promise<PendingOperation | null>((resolve) => {
      const request = store.get(id);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => resolve(null);
    });
    
    if (existing) {
      store.put({ ...existing, ...updates });
    }
  }

  async deletePendingOp(id: string): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['pendingOps'], 'readwrite');
    const store = tx.objectStore('pendingOps');
    store.delete(id);
  }

  // Storage management
  async getStorageInfo(): Promise<{ usage: number; quota: number }> {
    if ('storage' in navigator && 'estimate' in navigator.storage) {
      const estimate = await navigator.storage.estimate();
      return {
        usage: estimate.usage || 0,
        quota: estimate.quota || 0
      };
    }
    return { usage: 0, quota: 0 };
  }

  async purgeOldConversations(daysOld: number = 30): Promise<number> {
    await this.init();
    const cutoffDate = new Date();
    cutoffDate.setDate(cutoffDate.getDate() - daysOld);
    
    const conversations = await this.getAllConversations();
    let purged = 0;
    
    for (const conv of conversations) {
      const lastMessageDate = new Date(conv.updated_at);
      if (lastMessageDate < cutoffDate) {
        await this.deleteConversation(conv.id);
        purged++;
      }
    }
    
    return purged;
  }
}

// Singleton instance
export const cacheDB = new CacheDB();
