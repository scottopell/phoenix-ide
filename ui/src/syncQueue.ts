// Sync queue for handling offline operations

import { api } from './api';
import { cacheDB, type PendingOperation } from './cache';
import { notifyArchiveCloseConflict } from './notifications';

export class SyncQueue {
  async processOperation(op: PendingOperation): Promise<void> {
    switch (op.type) {
      case 'send_message': {
        if (!op.payload.localId) {
          throw new Error('send_message requires localId');
        }
        if (!op.payload.text && (op.payload.files || []).length === 0 && (op.payload.images || []).length === 0) {
          throw new Error('send_message requires text or attachments');
        }
        await api.sendMessage(
          op.conversationId,
          op.payload.text || '',
          op.payload.images || [],
          op.payload.files || [],
          op.payload.localId,
        );
        break;
      }
      case 'archive':
        try {
          await api.archiveConversation(op.conversationId);
        } catch (error) {
          if (!notifyArchiveCloseConflict(op.conversationId, error)) throw error;
        }
        break;
      
      case 'delete':
        await api.deleteConversation(op.conversationId);
        break;
      
      case 'rename':
        if (!op.payload.name) {
          throw new Error('rename requires name');
        }
        await api.renameConversation(
          op.conversationId,
          op.payload.name
        );
        break;

      case 'archive_chain':
        try {
          await api.archiveChain(op.conversationId);
        } catch (error) {
          if (!notifyArchiveCloseConflict(op.conversationId, error)) throw error;
        }
        break;

      case 'delete_chain':
        await api.deleteChain(op.conversationId);
        break;

      default: {
        // Legacy queue entries: `unarchive` / `unarchive_chain` ops that were
        // persisted in IndexedDB before archive became a terminal lifecycle
        // transition (the unarchive endpoints no longer exist). The cache DB
        // schema is unchanged, so such rows can survive an upgrade. Drain them
        // as no-ops — the conversation simply stays archived — so the row is
        // deleted and the offline banner clears, instead of throwing
        // `Unknown operation type` forever and wedging the pending-op queue.
        const legacyType = (op as { type: string }).type;
        if (legacyType === 'unarchive' || legacyType === 'unarchive_chain') {
          console.debug('Draining legacy queue op (archive is now terminal):', legacyType);
          break;
        }
        throw new Error(`Unknown operation type: ${(op as PendingOperation).type}`);
      }
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

export async function processAndDeletePendingOperation(op: PendingOperation): Promise<void> {
  await syncQueue.processOperation(op);
  await cacheDB.deletePendingOp(op.id);
}
