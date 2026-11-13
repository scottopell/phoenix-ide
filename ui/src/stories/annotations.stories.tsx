import type { Story } from '@ladle/react';
import { AnnotationDesignFixture, type AnnotationDesignState } from '../fixtures/annotations/renderFixture';

function stateStory(state: AnnotationDesignState): Story {
  return function AnnotationStory() { return <AnnotationDesignFixture state={state} />; };
}

export const Empty = stateStory('01-empty');
Empty.storyName = '01-empty';
export const Selected = stateStory('02-selected');
Selected.storyName = '02-selected';
export const Writing = stateStory('03-writing');
Writing.storyName = '03-writing';
export const Discard = stateStory('04-discard');
Discard.storyName = '04-discard';
export const OneAnnotation = stateStory('05-one-annotation');
OneAnnotation.storyName = '05-one-annotation';
export const SeveralAnnotations = stateStory('06-several-annotations');
SeveralAnnotations.storyName = '06-several-annotations';
export const LongComment = stateStory('07-long-comment');
LongComment.storyName = '07-long-comment';
export const Sending = stateStory('08-sending');
Sending.storyName = '08-sending';
export const FailedSend = stateStory('09-failed-send');
FailedSend.storyName = '09-failed-send';
export const Sent = stateStory('10-sent');
Sent.storyName = '10-sent';
