import { createRoot } from 'react-dom/client';
import { TouchReadingFixture } from './TouchReadingFixture';

// Standalone, single-mount fixture; deliberately no app/service-worker boot or StrictMode replay.
createRoot(document.getElementById('root')!).render(<TouchReadingFixture />);
