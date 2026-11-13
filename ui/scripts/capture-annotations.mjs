import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import process from 'node:process';
import { buildLadleStoryUrl, selectCaptureEntries } from './capture-ladle-surface.mjs';

const exec = promisify(execFile);
const root = fileURLToPath(new URL('../', import.meta.url));
const base = process.env.LADLE_URL ?? 'http://127.0.0.1:61123/';
const out = path.resolve(root, process.env.ANNOTATION_QA_OUT ?? 'qa-artifacts/annotations');
const session = `annotation-design-${process.pid}`;
const layouts = [{ name: 'desktop', width: 1440, height: 900 }, { name: 'mobile', width: 390, height: 844 }];
const themes = selectCaptureEntries([{ name: 'light' }, { name: 'dark' }], process.env.ANNOTATION_THEMES, 'name', 'ANNOTATION_THEMES');

async function browser(...args) {
  const { stdout } = await exec('phoenix-browser', ['--session', session, '--json', ...args], { maxBuffer: 8 * 1024 * 1024 });
  const response = JSON.parse(stdout);
  if (response.status !== 'success') throw new Error(response.error?.message ?? stdout);
  return response.result;
}

async function ensureLadle() {
  try { if ((await fetch(base)).ok) return null; } catch { /* Start the local harness below. */ }
  if (process.env.LADLE_URL) throw new Error(`Ladle is unavailable at ${base}`);
  const child = spawn('pnpm', ['ladle'], { cwd: root, stdio: ['ignore', 'inherit', 'inherit'], detached: true });
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline && child.exitCode === null) {
    try { if ((await fetch(base)).ok) return child; } catch { /* Wait for the listening socket. */ }
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  try { process.kill(-child.pid, 'SIGTERM'); } catch { /* Process already exited. */ }
  throw new Error('Ladle did not become ready');
}

function escape(value) {
  return String(value).replace(/[&<>"']/g, character => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]);
}

async function writeGallery(stories, captures) {
  const rows = themes.flatMap(theme => stories.map(story => {
    const cells = layouts.map(layout => {
      const capture = captures.find(item => item.id === story.id && item.theme === theme.name && item.layout === layout.name);
      return `<figure><figcaption>${layout.name} · ${layout.width} × ${layout.height} · composer ${Math.round(capture.composerHeight)}px (${Math.round(capture.composerHeight / layout.height * 100)}% of screen)</figcaption><a href="${escape(capture.file)}"><img loading="lazy" src="${escape(capture.file)}" width="${layout.width}" height="${layout.height}" alt="${escape(story.id)} ${layout.name} ${theme.name}"></a></figure>`;
    }).join('');
    const live = buildLadleStoryUrl(base, story.storyKey, { fixtureTheme: theme.name });
    return `<section data-theme="${theme.name}" data-state="${story.id}"><h2>${escape(story.id.replace(/^\d+-/, '').replaceAll('-', ' '))}</h2><a class="live" href="${escape(live)}" target="_blank" rel="noreferrer">Open live story ↗</a><div class="pair">${cells}</div></section>`;
  }));
  await writeFile(path.join(out, 'index.html'), `<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Annotation design states</title>
<style>
*{box-sizing:border-box}body{margin:0;background:#f0f2f5;color:#17212b;font:15px system-ui,sans-serif}header{position:sticky;top:0;z-index:2;background:#fffffff2;backdrop-filter:blur(12px);padding:16px 24px;border-bottom:1px solid #d4dbe3}h1{font-size:22px;margin:0 0 6px}header p{margin:0 0 12px;color:#526171}label{margin-right:18px}select{font:inherit;padding:5px}main{max-width:1560px;margin:auto;padding:16px 24px}section{position:relative;margin-bottom:24px;padding:18px;background:white;border:1px solid #d4dbe3;border-radius:12px}h2{margin:0 0 14px;text-transform:capitalize;font-size:18px}.live{position:absolute;right:18px;top:18px;color:#245fba}.pair{display:grid;grid-template-columns:minmax(0,2fr) minmax(220px,1fr);gap:20px;align-items:start}figure{margin:0;min-width:0}figcaption{font-size:12px;color:#526171;margin-bottom:8px}img{display:block;width:100%;height:auto;border:1px solid #d4dbe3;border-radius:6px}section[hidden]{display:none}@media(max-width:720px){.pair{grid-template-columns:1fr}main{padding:12px}.live{position:static;display:block;margin-bottom:12px}}
</style><header><h1>Annotation design states</h1><p>Production conversation components · generated ${new Date().toLocaleString('en-US', { timeZone: 'America/New_York' })} ET · click any screenshot for full resolution.</p>
<label>Theme <select id="theme">${themes.map(theme => `<option>${theme.name}</option>`).join('')}</select></label>
<label>State <select id="state"><option value="all">All states</option>${stories.map(story => `<option value="${story.id}">${escape(story.id.replace(/^\d+-/, '').replaceAll('-', ' '))}</option>`).join('')}</select></label></header><main>${rows.join('\n')}</main>
<script>const theme=document.querySelector('#theme'),state=document.querySelector('#state');function filter(){for(const section of document.querySelectorAll('main section'))section.hidden=section.dataset.theme!==theme.value||(state.value!=='all'&&section.dataset.state!==state.value)}theme.onchange=state.onchange=filter;filter();</script></html>`);
  await writeFile(path.join(out, 'captures.json'), JSON.stringify(captures, null, 2));
}

const server = await ensureLadle();
let cleaned = false;
async function cleanup() {
  if (cleaned) return;
  cleaned = true;
  await browser('close').catch(() => {});
  if (server) { try { process.kill(-server.pid, 'SIGTERM'); } catch { /* Already stopped. */ } }
}
process.once('SIGINT', () => { void cleanup().finally(() => process.exit(130)); });
process.once('SIGTERM', () => { void cleanup().finally(() => process.exit(143)); });
try {
  await mkdir(out, { recursive: true });
  const manifest = await (await fetch(new URL('meta.json', base))).json();
  const stories = selectCaptureEntries(Object.keys(manifest.stories).filter(key => key.startsWith('annotations--')).sort().map(storyKey => ({ storyKey, id: storyKey.slice('annotations--'.length) })), process.env.CAPTURE_STORIES, 'id', 'CAPTURE_STORIES');
  if (!stories.length) throw new Error('No annotation stories in the Ladle manifest');
  const captures = [];
  await browser('open', base);
  for (const theme of themes) for (const layout of layouts) {
    await browser('resize', String(layout.width), String(layout.height));
    for (const story of stories) {
      if (!/^[a-z0-9-]+$/.test(story.id)) throw new Error(`Invalid story id ${story.id}`);
      await browser('console-clear');
      await browser('open', buildLadleStoryUrl(base, story.storyKey, { fixtureTheme: theme.name }));
      await browser('wait', `html[data-annotation-design-ready="${story.id}"]`);
      const measurement = await browser('eval', `(()=>{const composer=document.querySelector('#input-area').getBoundingClientRect();return {composerHeight:composer.height,cards:document.querySelectorAll('.draft-annotation').length,highlights:CSS.highlights?.get('phoenix-pending-annotations')?.size??0,overflow:document.documentElement.scrollWidth>innerWidth}})()`);
      if (measurement.data?.kind !== 'json') throw new Error('Missing screenshot geometry');
      const metrics = measurement.data.value;
      if (metrics.overflow || metrics.composerHeight <= 0) throw new Error(`Invalid geometry for ${story.id}: ${JSON.stringify(metrics)}`);
      const logs = await browser('console', '--limit', '100');
      const errors = JSON.parse(logs.text).filter(entry => entry.level === 'error' && !(story.id === '09-failed-send' && entry.text.includes('Failed to send message:')));
      if (errors.length) throw new Error(`Unexpected console errors for ${story.id}: ${JSON.stringify(errors)}`);
      const file = `${story.id}--${layout.name}--${theme.name}.png`;
      await browser('screenshot', '--output', path.join(out, file));
      captures.push({ id: story.id, theme: theme.name, layout: layout.name, file, ...metrics });
      console.log(`✓ ${story.id} · ${layout.name} · ${theme.name} · composer ${Math.round(metrics.composerHeight)}px`);
    }
  }
  await writeGallery(stories, captures);
  console.log(`Gallery: ${path.join(out, 'index.html')}`);
  console.log(`Open in Ladle: ${new URL(`/@fs/${out.replaceAll('\\', '/')}/index.html`, base).href}`);
} finally {
  await cleanup();
}
