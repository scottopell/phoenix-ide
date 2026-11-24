import type { ProductConversationActionController } from '../hooks/useProductConversationActions';
import { ConfirmDialog } from './ConfirmDialog';
import { RenameDialog } from './RenameDialog';

export function ProductConversationActionDialogs({
  actions,
}: {
  actions: ProductConversationActionController;
}) {
  return (
    <>
      <ConfirmDialog
        visible={actions.closeTarget !== null}
        title="Close Conversation"
        message={`Close "${actions.closeTarget?.presentation.display_name}"? This moves the conversation to read-only History and stops its active work.`}
        confirmText="Close"
        danger
        onConfirm={() => void actions.confirmClose()}
        submitting={actions.closeSubmitting}
        {...(actions.closeError ? { error: actions.closeError } : {})}
        onCancel={actions.cancelClose}
      />
      <ConfirmDialog
        visible={actions.deleteTarget !== null}
        title="Delete Conversation"
        message="Permanently delete this conversation and all of its transcripts? This cannot be undone."
        confirmText="Delete"
        danger
        onConfirm={() => void actions.confirmDelete()}
        submitting={actions.deleteSubmitting}
        {...(actions.deleteError ? { error: actions.deleteError } : {})}
        onCancel={actions.cancelDelete}
      />
      {!actions.renameTarget && actions.renameError && (
        <div role="alert" className="error-message">{actions.renameError}</div>
      )}
      <RenameDialog
        visible={actions.renameTarget !== null}
        currentName={actions.renameTarget?.presentation.display_name ?? ''}
        error={actions.renameError}
        onRename={actions.confirmRename}
        onGenerate={actions.generateRename}
        normalizeInput={(value) => value}
        isValidName={(value) => value.trim().length > 0}
        helpText="Enter a title"
        maxLength={200}
        onCancel={actions.cancelRename}
      />
    </>
  );
}
