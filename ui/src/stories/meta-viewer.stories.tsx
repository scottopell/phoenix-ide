import type { Story } from '@ladle/react';
import { MetaViewerFixture, metaViewerScenarios } from '../fixtures/metaViewer';

const storyFor = (id: string): Story => {
  const scenario = metaViewerScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown meta-viewer scenario: ${id}`);
  return function MetaViewerStory() {
    return <MetaViewerFixture scenario={scenario} />;
  };
};

export const LargeTextFallbackDark = storyFor('large-text-fallback-dark');
LargeTextFallbackDark.storyName = 'large-text-fallback-dark';

export const LargeTextFallbackLight = storyFor('large-text-fallback-light');
LargeTextFallbackLight.storyName = 'large-text-fallback-light';

export const PatchContextDark = storyFor('patch-context-dark');
PatchContextDark.storyName = 'patch-context-dark';

export const HtmlSourceDark = storyFor('html-source-dark');
HtmlSourceDark.storyName = 'html-source-dark';

export const HtmlPreviewDark = storyFor('html-preview-dark');
HtmlPreviewDark.storyName = 'html-preview-dark';

export const ImageTakeoverDark = storyFor('image-takeover-dark');
ImageTakeoverDark.storyName = 'image-takeover-dark';

export const NotesPanelDark = storyFor('notes-panel-dark');
NotesPanelDark.storyName = 'notes-panel-dark';

export const AnnotationDialogDark = storyFor('annotation-dialog-dark');
AnnotationDialogDark.storyName = 'annotation-dialog-dark';

export const LoadingDark = storyFor('loading-dark');
LoadingDark.storyName = 'loading-dark';

export const ErrorDark = storyFor('error-dark');
ErrorDark.storyName = 'error-dark';
