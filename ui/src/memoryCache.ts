// Memory cache for hot data with stale-while-revalidate support

import type { Conversation, Message } from './api';
import type { CacheMeta } from './cache';

export interface CachedData<T> {
  data: T;
  meta: CacheMeta;
}

export interface CacheResult<T> {
  data: T;
  stale: boolean;
  age: number;
}

export class MemoryCache {
  private conversations = new Map<string, CachedData<Conversation>>();
  private conversationsBySlug = new Map<string, string>(); // slug -> id
  private messages = new Map<string, Message[]>(); // conversationId -> messages
  private allConversations: CachedData<Conversation[]> | null = null;
  private archivedConversations: CachedData<Conversation[]> | null = null;
  
  // Cache TTL settings
  private readonly maxAge = 5 * 60 * 1000; // 5 minutes
  private readonly maxMessageCount = 100; // Keep last N messages per conversation

  // Get with stale check
  getConversation(id: string): CacheResult<Conversation> | null {
    const cached = this.conversations.get(id);
    if (!cached) return null;
    
    const age = Date.now() - cached.meta.timestamp;
    return {
      data: cached.data,
      stale: age > this.maxAge,
      age
    };
  }

  getConversationBySlug(slug: string): CacheResult<Conversation> | null {
    const id = this.conversationsBySlug.get(slug);
    if (!id) return null;
    return this.getConversation(id);
  }

  getAllConversations(archived: boolean = false): CacheResult<Conversation[]> | null {
    const cached = archived ? this.archivedConversations : this.allConversations;
    if (!cached) return null;
    
    const age = Date.now() - cached.meta.timestamp;
    return {
      data: cached.data,
      stale: age > this.maxAge,
      age
    };
  }

  getMessages(conversationId: string): Message[] | null {
    return this.messages.get(conversationId) || null;
  }

  // Set methods
  setConversation(conversation: Conversation, meta?: Partial<CacheMeta>): void {
    const cacheMeta: CacheMeta = {
      timestamp: Date.now(),
      ...meta
    };
    
    this.conversations.set(conversation.id, {
      data: conversation,
      meta: cacheMeta
    });
    
    if (conversation.slug) {
      this.conversationsBySlug.set(conversation.slug, conversation.id);
    }
  }

  setAllConversations(conversations: Conversation[], archived: boolean = false, meta?: Partial<CacheMeta>): void {
    const cacheMeta: CacheMeta = {
      timestamp: Date.now(),
      ...meta
    };
    
    const cachedData = { data: conversations, meta: cacheMeta };
    
    if (archived) {
      this.archivedConversations = cachedData;
    } else {
      this.allConversations = cachedData;
    }
    
    // Also update individual conversation cache
    for (const conv of conversations) {
      this.setConversation(conv, meta);
    }
  }

  setMessages(conversationId: string, messages: Message[]): void {
    // Keep only last N messages to prevent unbounded growth
    const trimmed = messages.slice(-this.maxMessageCount);
    this.messages.set(conversationId, trimmed);
  }

  appendMessage(conversationId: string, message: Message): void {
    const existing = this.messages.get(conversationId) || [];
    const updated = [...existing, message].slice(-this.maxMessageCount);
    this.messages.set(conversationId, updated);
  }

  // Invalidation
  invalidateConversation(id: string): void {
    const conv = this.conversations.get(id);
    if (conv?.data.slug) {
      this.conversationsBySlug.delete(conv.data.slug);
    }
    this.conversations.delete(id);
    this.messages.delete(id);
    
    // Also invalidate list caches as they might contain this conversation
    this.allConversations = null;
    this.archivedConversations = null;
  }

  invalidateAll(): void {
    this.conversations.clear();
    this.conversationsBySlug.clear();
    this.messages.clear();
    this.allConversations = null;
    this.archivedConversations = null;
  }

  // Update specific fields without full invalidation
  updateConversation(id: string, updates: Partial<Conversation>): void {
    const cached = this.conversations.get(id);
    if (!cached) return;
    
    const updated = {
      ...cached,
      data: {
        ...cached.data,
        ...updates
      }
    };
    
    // Update slug mapping if slug changed
    if (updates.slug && updates.slug !== cached.data.slug) {
      if (cached.data.slug) {
        this.conversationsBySlug.delete(cached.data.slug);
      }
      this.conversationsBySlug.set(updates.slug, id);
    }
    
    this.conversations.set(id, updated);
    
    // Update in list caches if present
    if (this.allConversations) {
      this.allConversations.data = this.allConversations.data.map(conv => 
        conv.id === id ? updated.data : conv
      );
    }
    if (this.archivedConversations) {
      this.archivedConversations.data = this.archivedConversations.data.map(conv => 
        conv.id === id ? updated.data : conv
      );
    }
  }

  // Storage info
  getStats(): {
    conversationCount: number;
    messageCount: number;
    approximateSizeBytes: number;
  } {
    let messageCount = 0;
    for (const msgs of this.messages.values()) {
      messageCount += msgs.length;
    }
    
    // Rough estimate: 1KB per conversation, 0.5KB per message
    const approximateSizeBytes = 
      (this.conversations.size * 1024) + 
      (messageCount * 512);
    
    return {
      conversationCount: this.conversations.size,
      messageCount,
      approximateSizeBytes
    };
  }
}

// Singleton instance
export const memoryCache = new MemoryCache();
