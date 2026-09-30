import type { Story } from '@ladle/react';
import { MessageListFixture, getMessageListScenario, messageListScenarios } from '../fixtures/messageList';

const storyFor = (id: (typeof messageListScenarios)[number]['id']): Story => {
  const scenario = getMessageListScenario(id);
  return function MessageListStory() {
    return <MessageListFixture scenario={scenario} />;
  };
};

export const CompactLatestExpanded = storyFor('compact-latest-expanded');
CompactLatestExpanded.storyName = 'compact-latest-expanded';

export const CompactToolStrip = storyFor('compact-tool-strip');
CompactToolStrip.storyName = 'compact-tool-strip';

export const CompactExpandedToolChronology = storyFor('compact-expanded-tool-chronology');
CompactExpandedToolChronology.storyName = 'compact-expanded-tool-chronology';

export const ScrollPolicyLong = storyFor('scroll-policy-long');
ScrollPolicyLong.storyName = 'scroll-policy-long';

export const PrefixContinuityOffsetBug = storyFor('prefix-continuity-offset-bug');
PrefixContinuityOffsetBug.storyName = 'prefix-continuity-offset-bug';

export const MobileTablePreviewBaseline = storyFor('mobile-table-preview-baseline');
MobileTablePreviewBaseline.storyName = 'mobile-table-preview-baseline';

export const MobileTablePreviewContentWrap = storyFor('mobile-table-preview-content-wrap');
MobileTablePreviewContentWrap.storyName = 'mobile-table-preview-content-wrap';

export const MobileTablePreviewReadableOverflow = storyFor('mobile-table-preview-readable-overflow');
MobileTablePreviewReadableOverflow.storyName = 'mobile-table-preview-readable-overflow';

export const WideMarkdownTable = storyFor('wide-markdown-table');
WideMarkdownTable.storyName = 'wide-markdown-table';

export const WideMarkdownTableLight = storyFor('wide-markdown-table-light');
WideMarkdownTableLight.storyName = 'wide-markdown-table-light';

export const MarkdownImageDark = storyFor('markdown-image-dark');
MarkdownImageDark.storyName = 'markdown-image-dark';
