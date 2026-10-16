import { useEffect, useLayoutEffect, useMemo, useRef } from 'react';
import { useLocation, useNavigationType } from 'react-router-dom';
import type { ViewJourneyReporter } from './conversationViewTelemetry';
import { createViewJourneyReporter } from './conversationViewReporterFactory';
import { ViewJourneyLifecycle, defaultViewIdentity } from './conversationViewLifecycle';

let reporter: ViewJourneyReporter | undefined;
function tabReporter(): ViewJourneyReporter {
  return reporter ??= createViewJourneyReporter();
}

export function useConversationViewJourney(route: 'global' | 'ordinary', inherited?: ViewJourneyLifecycle): ViewJourneyLifecycle {
  const location = useLocation();
  const navigationType = useNavigationType();
  const first = useRef(true);
  const epoch = useRef(0);
  const admission = useMemo(() => {
    const navigation = performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming | undefined;
    const trigger = first.current && navigation?.type === 'reload' ? 'reload'
      : navigationType === 'POP' && (!first.current || navigation?.type === 'back_forward') ? 'back_forward' : 'navigation';
    return { navigationKey: location.key, lifecycle: inherited ?? new ViewJourneyLifecycle(defaultViewIdentity(route, trigger), tabReporter()) };
  }, [inherited, location.key, navigationType, route]);
  const lifecycle = admission.lifecycle;
  const currentAdmission = useRef(admission);
  useLayoutEffect(() => { currentAdmission.current = admission; }, [admission]);

  useEffect(() => {
    if (inherited) return;
    first.current = false;
    const attachEpoch = epoch.current + 1;
    epoch.current = attachEpoch;
    void lifecycle.openId;
    const standalone = defaultViewIdentity(route, 'navigation').standalone;
    let hidden = document.visibilityState !== 'visible';
    let suspended = false;
    let restored = false;
    const onVisibility = () => {
      const visible = document.visibilityState === 'visible';
      lifecycle.visibility(visible);
      if (visible && hidden && (standalone || restored)) {
        lifecycle.resume(standalone ? 'pwa_resume' : 'back_forward');
        suspended = restored = false;
      }
      hidden = !visible;
    };
    const onPageHide = () => {
      lifecycle.visibility(false);
      lifecycle.finish('incomplete');
      hidden = true;
      suspended = true;
      restored = false;
    };
    const onPageShow = (event: PageTransitionEvent) => {
      if (event.persisted && suspended) {
        restored = true;
        if (document.visibilityState === 'visible') {
          lifecycle.resume(standalone ? 'pwa_resume' : 'back_forward');
          hidden = suspended = restored = false;
        }
      }
    };
    document.addEventListener('visibilitychange', onVisibility);
    window.addEventListener('pagehide', onPageHide);
    window.addEventListener('pageshow', onPageShow);
    return () => {
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('pagehide', onPageHide);
      window.removeEventListener('pageshow', onPageShow);
      queueMicrotask(() => {
        if (currentAdmission.current.navigationKey !== admission.navigationKey || currentAdmission.current.lifecycle !== lifecycle || epoch.current === attachEpoch) lifecycle.finish('canceled');
      });
    };
  }, [admission, inherited, lifecycle, route]);
  return lifecycle;
}
