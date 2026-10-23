import path from 'node:path';
import react from '../../ui/node_modules/@vitejs/plugin-react/dist/index.js';
const root=path.resolve('ui');
export default {root,plugins:[react()],cacheDir:path.join(process.env.RIG_WORK,'cache'),publicDir:false,css:{postcss:{plugins:[]}},server:{host:'127.0.0.1',port:4179,strictPort:true,open:false,hmr:false,fs:{allow:[root]}},optimizeDeps:{entries:['src/fixtures/touchReading/index.html']}};
