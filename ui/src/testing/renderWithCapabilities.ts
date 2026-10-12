import { createElement, type ComponentType, type ReactNode } from 'react';
import * as rtl from '@testing-library/react';
import { LiveApplicationCapabilitiesFixture } from '../fixtures/LiveApplicationCapabilitiesFixture';

export * from '@testing-library/react';

function fixtureWrapper(Wrapper?: ComponentType<{ children: ReactNode }>) {
  return function CapabilitiesWrapper({ children }: { children: ReactNode }) {
    return createElement(LiveApplicationCapabilitiesFixture, null, Wrapper ? createElement(Wrapper, null, children) : children);
  };
}

export function render(ui: ReactNode, options?: rtl.RenderOptions) {
  return rtl.render(ui, { ...options, wrapper: fixtureWrapper(options?.wrapper) });
}

export function renderHook<Result, Props>(callback: (props: Props) => Result, options?: rtl.RenderHookOptions<Props>) {
  return rtl.renderHook(callback, { ...options, wrapper: fixtureWrapper(options?.wrapper) });
}
