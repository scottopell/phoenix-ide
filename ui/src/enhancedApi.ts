// Enhanced API client with caching and offline support

import { api as baseApi, Conversation, Message, ModelsResponse } from './api';
import { cacheDB } from './cache';
import { memoryCache } from './memoryCache';
import type { ImageData } from './api';

export interface FetchOptions {
  // Force fresh fetch even if cached data exists
  forceFresh?: boolean;
  // Skip cache write after fetch
  skipCache?: boolean;
  // Custom stale time (ms)
  maxAge?: number;
}

export interface CachedResponse<T> {
  data: T;
  source: 'memory' | 'indexeddb' | 'network';
  stale: boolean;
  age: number;
}

class EnhancedAPI {
  private inFlightRequests = new Map<string, Promise<any>>();
  
  // Request deduplication helper
  private async dedupe<T>(
    key: string,
    fetcher: () => Promise<T>
  ): Promise<T> {
    const existing = this.inFlightRequests.get(key);
    if (existing) {
      return existing;
    }
    
    const promise = fetcher().finally(() => {
      this.inFlightRequests.delete(key);
    });
    
    this.inFlightRequests.set(key, promise);
    return promise;
  }
  
  // Conversations
  async listConversations(options: FetchOptions = {}): Promise<CachedResponse<Conversation[]>> {
    const cacheKey = 'conversations:active';
    
    // Check memory cache first
    if (!options.forceFresh) {
      const memResult = memoryCache.getAllConversations(false);
      if (memResult) {
        // Return immediately, but trigger background refresh if stale
        if (memResult.stale) {
          this.listConversations({ ...options, forceFresh: true }).catch(console.error);
        }
        return {
          data: memResult.data,
          source: 'memory',
          stale: memResult.stale,
          age: memResult.age
        };
      }
    }
    
    // Check IndexedDB
    if (!options.forceFresh) {
      const cachedConvs = await cacheDB.getAllConversations();
      const active = cachedConvs.filter(c => !c.archived);
      if (active.length > 0) {
        const age = Math.min(...active.map(c => Date.now() - c._meta.timestamp));
        const stale = age > (options.maxAge || 5 * 60 * 1000);
        
        // Update memory cache
        memoryCache.setAllConversations(active, false);
        
        // Background refresh if stale
        if (stale) {
          this.listConversations({ ...options, forceFresh: true }).catch(console.error);
        }
        
        return {
          data: active,
          source: 'indexeddb',
          stale,
          age
        };
      }
    }
    
    // Fetch from network
    return this.dedupe(cacheKey, async () => {
      const conversations = await baseApi.listConversations();
      
      // Update caches
      if (!options.skipCache) {
        memoryCache.setAllConversations(conversations, false);
        for (const conv of conversations) {
          await cacheDB.putConversation(conv);
        }
      }
      
      return {
        data: conversations,
        source: 'network' as const,
        stale: false,
        age: 0
      };
    });
  }
  
  async listArchivedConversations(options: FetchOptions = {}): Promise<CachedResponse<Conversation[]>> {
    const cacheKey = 'conversations:archived';
    
    // Similar pattern to listConversations
    if (!options.forceFresh) {
      const memResult = memoryCache.getAllConversations(true);
      if (memResult) {
        if (memResult.stale) {
          this.listArchivedConversations({ ...options, forceFresh: true }).catch(console.error);
        }
        return {
          data: memResult.data,
          source: 'memory',
          stale: memResult.stale,
          age: memResult.age
        };
      }
    }
    
    return this.dedupe(cacheKey, async () => {
      const conversations = await baseApi.listArchivedConversations();
      
      if (!options.skipCache) {
        memoryCache.setAllConversations(conversations, true);
        for (const conv of conversations) {
          await cacheDB.putConversation(conv);
        }
      }
      
      return {
        data: conversations,
        source: 'network' as const,
        stale: false,
        age: 0
      };
    });
  }
  
  async getConversationBySlug(
    slug: string,
    options: FetchOptions = {}
  ): Promise<CachedResponse<{ conversation: Conversation; messages: Message[]; agent_working: boolean; context_window_size: number }>> {
    const cacheKey = `conversation:slug:${slug}`;
    
    // Check memory cache
    if (!options.forceFresh) {
      const convResult = memoryCache.getConversationBySlug(slug);
      const messages = convResult ? memoryCache.getMessages(convResult.data.id) : null;
      
      if (convResult && messages) {
        // We have cached data
        if (convResult.stale) {
          // Trigger background refresh
          this.getConversationBySlug(slug, { ...options, forceFresh: true }).catch(console.error);
        }
        
        return {
          data: {
            conversation: convResult.data,
            messages,
            agent_working: convResult.data.state?.type === 'processing',
            context_window_size: 0 // This is dynamic, can't cache reliably
          },
          source: 'memory',
          stale: convResult.stale,
          age: convResult.age
        };
      }
    }
    
    // Check IndexedDB
    if (!options.forceFresh) {
      const cachedConv = await cacheDB.getConversationBySlug(slug);
      if (cachedConv) {
        const messages = await cacheDB.getMessages(cachedConv.id);
        const age = Date.now() - cachedConv._meta.timestamp;
        const stale = age > (options.maxAge || 5 * 60 * 1000);
        
        // Update memory cache
        memoryCache.setConversation(cachedConv);
        memoryCache.setMessages(cachedConv.id, messages);
        
        if (stale) {
          this.getConversationBySlug(slug, { ...options, forceFresh: true }).catch(console.error);
        }
        
        return {
          data: {
            conversation: cachedConv,
            messages,
            agent_working: cachedConv.state?.type === 'processing',
            context_window_size: 0
          },
          source: 'indexeddb',
          stale,
          age
        };
      }
    }
    
    // Fetch from network
    return this.dedupe(cacheKey, async () => {
      const result = await baseApi.getConversationBySlug(slug);
      
      // Update caches
      if (!options.skipCache) {
        const scrollPos = memoryCache.getConversationBySlug(slug)?.data._meta?.scrollPosition;
        memoryCache.setConversation(result.conversation, { scrollPosition: scrollPos });
        memoryCache.setMessages(result.conversation.id, result.messages);
        
        await cacheDB.putConversation(result.conversation, { scrollPosition: scrollPos });
        await cacheDB.putMessages(result.messages);
      }
      
      return {
        data: result,
        source: 'network' as const,
        stale: false,
        age: 0
      };
    });
  }
  
  // Operations that modify data (these invalidate caches)
  async createConversation(cwd: string, model?: string): Promise<Conversation> {
    const conversation = await baseApi.createConversation(cwd, model);
    
    // Add to caches
    memoryCache.setConversation(conversation);
    await cacheDB.putConversation(conversation);
    
    // Invalidate list caches
    memoryCache.setAllConversations(
      [...(memoryCache.getAllConversations()?.data || []), conversation],
      false
    );
    
    return conversation;
  }
  
  async sendMessage(convId: string, text: string, images: ImageData[] = []): Promise<{ queued: boolean }> {
    // For offline support, this will be handled by the app machine
    return baseApi.sendMessage(convId, text, images);
  }
  
  async archiveConversation(convId: string): Promise<void> {
    await baseApi.archiveConversation(convId);
    
    // Update local state immediately
    const conv = memoryCache.getConversation(convId)?.data;
    if (conv) {
      memoryCache.updateConversation(convId, { archived: true });
      await cacheDB.putConversation({ ...conv, archived: true });
    }
    
    // Invalidate list caches
    memoryCache.setAllConversations(null as any, false);
    memoryCache.setAllConversations(null as any, true);
  }
  
  async unarchiveConversation(convId: string): Promise<void> {
    await baseApi.unarchiveConversation(convId);
    
    // Update local state immediately
    const conv = memoryCache.getConversation(convId)?.data;
    if (conv) {
      memoryCache.updateConversation(convId, { archived: false });
      await cacheDB.putConversation({ ...conv, archived: false });
    }
    
    // Invalidate list caches
    memoryCache.setAllConversations(null as any, false);
    memoryCache.setAllConversations(null as any, true);
  }
  
  async deleteConversation(convId: string): Promise<void> {
    await baseApi.deleteConversation(convId);
    
    // Remove from caches
    memoryCache.invalidateConversation(convId);
    await cacheDB.deleteConversation(convId);
  }
  
  async renameConversation(convId: string, name: string): Promise<void> {
    await baseApi.renameConversation(convId, name);
    
    // Update local state
    memoryCache.updateConversation(convId, { slug: name });
    const conv = memoryCache.getConversation(convId)?.data;
    if (conv) {
      await cacheDB.putConversation({ ...conv, slug: name });
    }
  }
  
  // Other endpoints (no caching for now)
  async cancelConversation(convId: string): Promise<{ ok: boolean }> {
    return baseApi.cancelConversation(convId);
  }
  
  async validateCwd(path: string): Promise<{ valid: boolean; error?: string }> {
    return baseApi.validateCwd(path);
  }
  
  async listDirectory(path: string): Promise<{ entries: { name: string; is_dir: boolean }[] }> {
    return baseApi.listDirectory(path);
  }
  
  async listModels(): Promise<ModelsResponse> {
    // Models don't change often, could cache for longer
    return baseApi.listModels();
  }
  
  // SSE streaming - no changes needed
  streamConversation = baseApi.streamConversation;
  
  // Save scroll position for a conversation
  async saveScrollPosition(slug: string, position: number): Promise<void> {
    const conv = memoryCache.getConversationBySlug(slug)?.data;
    if (conv) {
      memoryCache.setConversation(conv, { scrollPosition: position });
      await cacheDB.putConversation(conv, { scrollPosition: position });
    }
  }
}

export const enhancedApi = new EnhancedAPI();
