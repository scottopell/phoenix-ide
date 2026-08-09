import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import * as serviceWorkerRegistration from './serviceWorkerRegistration';
import { installDeploymentModuleRecovery } from './deploymentModuleRecovery';
import { ModuleLoadErrorBoundary } from './ModuleLoadErrorBoundary';

installDeploymentModuleRecovery();

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ModuleLoadErrorBoundary>
      <App />
    </ModuleLoadErrorBoundary>
  </React.StrictMode>,
);

// Register service worker for offline functionality
serviceWorkerRegistration.register();
