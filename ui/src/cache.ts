// Phoenix IDE Cache Layer using IndexedDB

import type { Conversation, Message } from './api';

const DB_NAME = 'phoenix-ide-cache';
const DB_VERSION = 1;

export interface CacheMeta {
  timestamp: number;
  etag?: string;
  scrollPosition?: number;
}

export interface CachedConversation extends Conversation {
  _meta: CacheMeta;
}

export interface PendingOperation {
  id: string;
  type: 'send_message' | 'archive' | 'unarchive' | 'delete' | 'rename';
  conversationId: string;
  payload: any;
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

        // Cache metadata
        if (!db.objectStoreNames.contains('cacheMeta')) {
          db.createObjectStore('cacheMeta', { keyPath: 'key' });
        }
      };
    });
  }

  // Conversations
  async getConversation(id: string): Promise<CachedConversation | null> {
    await this.init();
    const tx = this.db!.transaction(['conversations'], 'readonly');
    const store = tx.objectStore('conversations');
    return new Promise((resolve) => {
      const request = store.get(id);
      request.onsuccess = () => resolve(request.result || null);
      request.onerror = () => resolve(null);
    });
  }

  async getConversationBySlug(slug: string): Promise<CachedConversation | null> {
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

  async getAllConversations(): Promise<CachedConversation[]> {
    await this.init();
    const tx = this.db!.transaction(['conversations'], 'readonly');
    const store = tx.objectStore('conversations');
    return new Promise((resolve) => {
      const request = store.getAll();
      request.onsuccess = () => resolve(request.result || []);
      request.onerror = () => resolve([]);
    });
  }

  async putConversation(conversation: Conversation, meta?: Partial<CacheMeta>): Promise<void> {
    await this.init();
    const existing = await this.getConversation(conversation.id);
    const cached: CachedConversation = {
      ...conversation,
      _meta: {
        timestamp: Date.now(),
        ...(existing?._meta || {}),
        ...meta
      }
    };

    const tx = this.db!.transaction(['conversations'], 'readwrite');
    const store = tx.objectStore('conversations');
    store.put(cached);
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

  // Messages
  async getMessages(conversationId: string, afterSequence?: number): Promise<Message[]> {
    await this.init();
    const tx = this.db!.transaction(['messages'], 'readonly');
    const store = tx.objectStore('messages');
    const index = store.index('by-conversation');
    
    return new Promise((resolve) => {
      const messages: Message[] = [];
      const range = IDBKeyRange.only(conversationId);
      const request = index.openCursor(range);
      
      request.onsuccess = () => {
        const cursor = request.result;
        if (cursor) {
          const msg = cursor.value as Message;
          if (!afterSequence || msg.sequence_id > afterSequence) {
            messages.push(msg);
          }
          cursor.continue();
        } else {
          resolve(messages.sort((a, b) => a.sequence_id - b.sequence_id));
        }
      };
      
      request.onerror = () => resolve([]);
    });
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
    const id = crypto.randomUUID();
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

  // Cache metadata (for storing global state like last sync time)
  async getMeta(key: string): Promise<any> {
    await this.init();
    const tx = this.db!.transaction(['cacheMeta'], 'readonly');
    const store = tx.objectStore('cacheMeta');
    return new Promise((resolve) => {
      const request = store.get(key);
      request.onsuccess = () => resolve(request.result?.value);
      request.onerror = () => resolve(null);
    });
  }

  async setMeta(key: string, value: any): Promise<void> {
    await this.init();
    const tx = this.db!.transaction(['cacheMeta'], 'readwrite');
    const store = tx.objectStore('cacheMeta');
    store.put({ key, value });
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
