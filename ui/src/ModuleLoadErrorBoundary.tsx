import { Component, type ErrorInfo, type ReactNode } from 'react';
import './ModuleLoadErrorBoundary.css';

type Props = {
  children: ReactNode;
};

type State = {
  failed: boolean;
};

export class ModuleLoadErrorBoundary extends Component<Props, State> {
  override state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error('[Phoenix] A lazy UI module failed to initialize.', error, info.componentStack);
  }

  override render(): ReactNode {
    if (this.state.failed) {
      return (
        <main className="module-load-fallback" role="alert">
          <div className="module-load-fallback__title">Part of Phoenix could not be loaded</div>
          <div className="module-load-fallback__detail">The failed view was stopped safely. Reload when ready; unsent attachments will be lost.</div>
          <button className="module-load-fallback__reload" type="button" onClick={() => window.location.reload()}>Reload Phoenix</button>
        </main>
      );
    }

    return this.props.children;
  }
}
