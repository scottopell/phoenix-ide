import { lazy, Suspense, type ComponentProps } from 'react';
import type { Message } from '../../api';
import type { OpenFileState } from '../FileExplorer/fileExplorerTypes';
import type { ViewerPresentation, ViewerSlot } from '../../contexts/ViewerSlotContext';
import type { ConversationDiffViewer as ConversationDiffViewerType } from './ConversationDiffViewer';
import type { useResizablePane } from '../../hooks';

const FileViewer = lazy(() =>
  import('../FileViewer').then((m) => ({ default: m.FileViewer })),
);
const ConversationDiffViewer = lazy(() =>
  import('./ConversationDiffViewer').then((m) => ({ default: m.ConversationDiffViewer })),
);
const BrowserViewPanel = lazy(() =>
  import('../BrowserViewPanel').then((m) => ({ default: m.BrowserViewPanel })),
);
const ProcessInspectorPanel = lazy(() =>
  import('../ProcessInspectorPanel').then((m) => ({ default: m.ProcessInspectorPanel })),
);
const MessageViewer = lazy(() =>
  import('../MessageViewer').then((m) => ({ default: m.MessageViewer })),
);

type DiffProps = ComponentProps<typeof ConversationDiffViewerType>;
type InspectSlot = Extract<ViewerSlot, { kind: 'inspect' }>;
type MessageSlot = Extract<ViewerSlot, { kind: 'message' }>;
type ProseSlot = Extract<ViewerSlot, { kind: 'prose' }>;

export interface ConversationViewerHostProps {
  conversationId: string | undefined;
  isDesktop: boolean;
  isWideDesktop: boolean;
  showSplitPaneViewer: boolean;
  openFileState: OpenFileState | null;
  proseSlot: ProseSlot | null;
  paneDiffOpen: boolean;
  fullscreenDiffOpen: boolean;
  diffTarget: DiffProps['target'];
  activePrIdentity: DiffProps['activePrIdentity'];
  browserViewerOpen: boolean;
  inspectViewerOpen: boolean;
  inspectSlot: InspectSlot | null;
  messageViewerOpen: boolean;
  messageSlot: MessageSlot | null;
  viewableMessages: Message[];
  writableComposerMounted: boolean;
  onCloseFileViewer: () => void;
  onCloseDiff: () => void;
  onCloseBrowserView: () => void;
  onCloseInspector: () => void;
  onCloseMessageViewer: () => void;
  onSendNotes: (formatted: string) => void;
  onSendFocusedNotes: (formatted: string) => void;
  onPresentationChange: (presentation: ViewerPresentation) => void;
}

/** Narrow desktop (1025-1279px): active viewer replaces conversation content full-screen. */
export function NarrowDesktopViewerHost(props: ConversationViewerHostProps) {
  const {
    conversationId,
    openFileState,
    paneDiffOpen,
    diffTarget,
    activePrIdentity,
    browserViewerOpen,
    messageViewerOpen,
    messageSlot,
    viewableMessages,
    onCloseFileViewer,
    onCloseDiff,
    onCloseBrowserView,
    onCloseMessageViewer,
    onSendNotes,
  } = props;

  if (openFileState) {
    return (
      <div id="app">
        <Suspense fallback={null}>
          <FileViewer
            filePath={openFileState.path}
            rootDir={openFileState.rootDir}
            onClose={onCloseFileViewer}
            onSendNotes={onSendNotes}
            patchContext={openFileState.patchContext ?? undefined}
            focus={openFileState.focus}
            inline
          />
        </Suspense>
      </div>
    );
  }
  if (paneDiffOpen && conversationId) {
    return (
      <div id="app">
        <Suspense fallback={null}>
          <ConversationDiffViewer
            conversationId={conversationId}
            target={diffTarget}
            activePrIdentity={activePrIdentity}
            onClose={onCloseDiff}
            onSendNotes={onSendNotes}
            inline
          />
        </Suspense>
      </div>
    );
  }
  if (browserViewerOpen && conversationId) {
    return (
      <div id="app">
        <Suspense fallback={null}>
          <BrowserViewPanel
            conversationId={conversationId}
            onClose={onCloseBrowserView}
            inline
            takeover
          />
        </Suspense>
      </div>
    );
  }
  if (messageViewerOpen && messageSlot) {
    return (
      <div id="app">
        <Suspense fallback={null}>
          <MessageViewer
            sequenceId={messageSlot.sequenceId}
            messages={viewableMessages}
            onClose={onCloseMessageViewer}
            onSendNotes={onSendNotes}
            inline
          />
        </Suspense>
      </div>
    );
  }
  return null;
}

/** Mobile / fallback overlays and fullscreen diff takeover. */
export function ConversationViewerOverlays(props: ConversationViewerHostProps) {
  const {
    conversationId,
    isDesktop,
    isWideDesktop,
    showSplitPaneViewer,
    openFileState,
    paneDiffOpen,
    fullscreenDiffOpen,
    diffTarget,
    activePrIdentity,
    browserViewerOpen,
    inspectViewerOpen,
    inspectSlot,
    messageViewerOpen,
    messageSlot,
    viewableMessages,
    writableComposerMounted,
    onCloseFileViewer,
    onCloseDiff,
    onCloseBrowserView,
    onCloseInspector,
    onCloseMessageViewer,
    onSendNotes,
    onSendFocusedNotes,
    onPresentationChange,
  } = props;

  return (
    <>
      {!isDesktop && openFileState && (
        <Suspense fallback={null}>
          <FileViewer
            filePath={openFileState.path}
            rootDir={openFileState.rootDir}
            onClose={onCloseFileViewer}
            onSendNotes={onSendNotes}
            patchContext={openFileState.patchContext ?? undefined}
            focus={openFileState.focus}
          />
        </Suspense>
      )}
      {fullscreenDiffOpen && conversationId && (
        <Suspense fallback={null}>
          <ConversationDiffViewer
            conversationId={conversationId}
            target={diffTarget}
            activePrIdentity={activePrIdentity}
            onClose={onCloseDiff}
            onSendNotes={onSendNotes}
            takeover
          />
        </Suspense>
      )}
      {paneDiffOpen && !showSplitPaneViewer && conversationId && (
        <Suspense fallback={null}>
          <ConversationDiffViewer
            conversationId={conversationId}
            target={diffTarget}
            activePrIdentity={activePrIdentity}
            onClose={onCloseDiff}
            onSendNotes={onSendNotes}
          />
        </Suspense>
      )}
      {browserViewerOpen && !showSplitPaneViewer && conversationId && (
        <Suspense fallback={null}>
          <div className="browser-view-overlay">
            <BrowserViewPanel
              conversationId={conversationId}
              onClose={onCloseBrowserView}
            />
          </div>
        </Suspense>
      )}
      {inspectViewerOpen && inspectSlot && !showSplitPaneViewer && (
        <Suspense fallback={null}>
          <ProcessInspectorPanel
            handleId={inspectSlot.handleId}
            conversationId={conversationId}
            onClose={onCloseInspector}
          />
        </Suspense>
      )}
      {messageViewerOpen && messageSlot && !showSplitPaneViewer && (
        <Suspense fallback={null}>
          <MessageViewer
            sequenceId={messageSlot.sequenceId}
            messages={viewableMessages}
            onClose={onCloseMessageViewer}
            onSendNotes={writableComposerMounted
              ? (isWideDesktop && messageSlot.presentation === 'fullscreen' ? onSendFocusedNotes : onSendNotes)
              : undefined}
            presentation={messageSlot.presentation}
            canTogglePresentation={isWideDesktop}
            onPresentationChange={onPresentationChange}
          />
        </Suspense>
      )}
    </>
  );
}

export interface ConversationViewerSplitPaneProps extends ConversationViewerHostProps {
  viewerPane: ReturnType<typeof useResizablePane>;
  minSize: number;
  maxSize: number;
  onLiveResize: (size: number, collapsed: boolean) => void;
}

export const VIEWER_PANE_MIN = 360;
export const VIEWER_PANE_MAX = 1200;

export function ViewerPaneDivider({
  viewerPane,
  minSize = VIEWER_PANE_MIN,
  maxSize = VIEWER_PANE_MAX,
  onLiveResize,
}: {
  viewerPane: ReturnType<typeof useResizablePane>;
  minSize?: number;
  maxSize?: number;
  onLiveResize: (size: number, collapsed: boolean) => void;
}) {
  return (
    <div
      className="viewer-pane-divider"
      title="Drag to resize the viewer pane • Double-click to collapse"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize viewer pane"
      aria-valuemin={minSize}
      aria-valuemax={maxSize}
      aria-valuenow={viewerPane.collapsed ? 0 : viewerPane.size}
      tabIndex={0}
      onPointerDown={(e) => viewerPane.startDrag(e, 'x', true, onLiveResize)}
      onDoubleClick={() => viewerPane.setCollapsed(!viewerPane.collapsed)}
      onKeyDown={(e) => {
        const STEP = 32;
        if (e.key === 'ArrowLeft') {
          e.preventDefault();
          viewerPane.setSize(viewerPane.size + STEP);
        } else if (e.key === 'ArrowRight') {
          e.preventDefault();
          viewerPane.setSize(viewerPane.size - STEP);
        } else if (e.key === 'Home') {
          e.preventDefault();
          viewerPane.setSize(maxSize);
        } else if (e.key === 'End') {
          e.preventDefault();
          viewerPane.setSize(minSize);
        } else if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          viewerPane.setCollapsed(!viewerPane.collapsed);
        }
      }}
    />
  );
}

/** Wide-desktop split pane + keyboard-accessible resize separator. */
export function ConversationViewerSplitPane(props: ConversationViewerSplitPaneProps) {
  const {
    conversationId,
    isWideDesktop,
    showSplitPaneViewer,
    openFileState: splitPanePrs,
    proseSlot,
    paneDiffOpen,
    diffTarget,
    activePrIdentity,
    browserViewerOpen,
    inspectViewerOpen,
    inspectSlot,
    messageViewerOpen,
    messageSlot,
    viewableMessages,
    writableComposerMounted,
    onCloseFileViewer,
    onCloseDiff,
    onCloseBrowserView,
    onCloseInspector,
    onCloseMessageViewer,
    onSendNotes,
    onSendFocusedNotes,
    onPresentationChange,
    viewerPane,
    minSize,
    maxSize,
    onLiveResize,
  } = props;

  if (!showSplitPaneViewer) return null;

  return (
    <>
      <ViewerPaneDivider
        viewerPane={viewerPane}
        minSize={minSize}
        maxSize={maxSize}
        onLiveResize={onLiveResize}
      />
      <div className="conversation-viewer-pane">
        <Suspense fallback={null}>
          {paneDiffOpen && conversationId ? (
            <ConversationDiffViewer
              conversationId={conversationId}
              target={diffTarget}
              activePrIdentity={activePrIdentity}
              onClose={onCloseDiff}
              onSendNotes={onSendNotes}
              inline
            />
          ) : splitPanePrs ? (
            <FileViewer
              filePath={splitPanePrs.path}
              rootDir={splitPanePrs.rootDir}
              onClose={onCloseFileViewer}
              onSendNotes={isWideDesktop && proseSlot?.presentation === 'fullscreen' ? onSendFocusedNotes : onSendNotes}
              patchContext={splitPanePrs.patchContext ?? undefined}
              focus={splitPanePrs.focus}
              presentation={proseSlot?.presentation ?? 'pane'}
              canTogglePresentation
              onPresentationChange={onPresentationChange}
              inline
            />
          ) : browserViewerOpen && conversationId ? (
            <BrowserViewPanel
              conversationId={conversationId}
              onClose={onCloseBrowserView}
              inline
            />
          ) : inspectViewerOpen && inspectSlot ? (
            <ProcessInspectorPanel
              handleId={inspectSlot.handleId}
              conversationId={conversationId}
              onClose={onCloseInspector}
              inline
            />
          ) : messageViewerOpen && messageSlot ? (
            <MessageViewer
              sequenceId={messageSlot.sequenceId}
              messages={viewableMessages}
              onClose={onCloseMessageViewer}
              onSendNotes={writableComposerMounted
                ? (isWideDesktop && messageSlot.presentation === 'fullscreen' ? onSendFocusedNotes : onSendNotes)
                : undefined}
              presentation={messageSlot.presentation}
              canTogglePresentation
              onPresentationChange={onPresentationChange}
              inline
            />
          ) : null}
        </Suspense>
      </div>
    </>
  );
}
