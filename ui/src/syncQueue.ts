// Sync queue for handling offline operations

import { api } from './api';
import type { PendingOperation } from './cache';

export class SyncQueue {
  async processOperation(op: PendingOperation): Promise<void> {
    switch (op.type) {
      case 'send_message':
        await api.sendMessage(
          op.conversationId,
          op.payload.text,
          op.payload.images || [],
          op.payload.localId,
        );
        break;
      
      case 'archive':
        await api.archiveConversation(op.conversationId);
        break;
      
      case 'unarchive':
        await api.unarchiveConversation(op.conversationId);
        break;
      
      case 'delete':
        await api.deleteConversation(op.conversationId);
        break;
      
      case 'rename':
        await api.renameConversation(
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
      return true;
    }
    
    if (error instanceof Error) {
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
