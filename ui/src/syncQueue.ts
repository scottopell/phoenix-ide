// Sync queue for handling offline operations

import { enhancedApi } from './enhancedApi';
import type { PendingOperation } from './cache';

export class SyncQueue {
  async processOperation(op: PendingOperation): Promise<void> {
    switch (op.type) {
      case 'send_message':
        await enhancedApi.sendMessage(
          op.conversationId,
          op.payload.text,
          op.payload.images || []
        );
        break;
      
      case 'archive':
        await enhancedApi.archiveConversation(op.conversationId);
        break;
      
      case 'unarchive':
        await enhancedApi.unarchiveConversation(op.conversationId);
        break;
      
      case 'delete':
        await enhancedApi.deleteConversation(op.conversationId);
        break;
      
      case 'rename':
        await enhancedApi.renameConversation(
          op.conversationId,
          op.payload.name
        );
        break;
      
      default:
        throw new Error(`Unknown operation type: ${(op as any).type}`);
    }
  }
  
  isRetryableError(error: unknown): boolean {
    if (error instanceof TypeError && error.message.includes('fetch')) {
      // Network errors
      return true;
    }
    
    if (error instanceof Error) {
      // Look for common retryable patterns
      const message = error.message.toLowerCase();
      if (
        message.includes('network') ||
        message.includes('timeout') ||
        message.includes('503') ||
        message.includes('502') ||
        message.includes('504')
      ) {
        return true;
      }
    }
    
    return false;
  }
}

export const syncQueue = new SyncQueue();
