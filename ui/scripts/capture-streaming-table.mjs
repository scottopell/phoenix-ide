// Existing Ladle + installed Playwright browser only; never starts/installs services.
// LADLE_URL=http://127.0.0.1:61123 node ui/scripts/capture-streaming-table.mjs
// STREAMING_TABLE_QA_BASELINE=1 requires a table escaping prose (pre-fix reproduction).
import { chromium, webkit } from 'playwright';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';

assert.ok(process.env.LADLE_URL, 'LADLE_URL must name an already-running Ladle');
const baseline = process.env.STREAMING_TABLE_QA_BASELINE === '1';
const engine = process.env.STREAMING_TABLE_QA_BROWSER ?? 'chromium';
assert.ok(['chromium', 'webkit'].includes(engine), 'Browser must be chromium or webkit');
const out = path.resolve(process.env.STREAMING_TABLE_QA_OUT_DIR ?? `qa-artifacts/streaming-table${baseline ? '-baseline' : ''}`);
await mkdir(out, { recursive: true });
const source = await readFile(new URL('../src/fixtures/assistantFinalization/acl-reply.md', import.meta.url), 'utf8');
const expectedCells = source.split('\n').filter(line => line.startsWith('|') && !line.startsWith('|---'))
  .flatMap(line => line.slice(1, -1).split('|').map(cell => cell.trim().replace(/[`*]/g, '')));
const samples = [];
const failures = [];
const errors = [];
let breakoutCount = 0;
const check = (condition, message) => { if (!condition) failures.push(message); };
const near = (a, b) => Math.abs(a - b) <= 1;

function measure() {
  const rect = node => {
    const r = node.getBoundingClientRect();
    return { left: r.left, right: r.right, top: r.top, width: r.width, height: r.height };
  };
  const size = node => ({ ...rect(node), clientWidth: node.clientWidth, scrollWidth: node.scrollWidth });
  const fixture = document.querySelector('.streaming-table-fixture');
  const chat = document.querySelector('#chat-view');
  const style = getComputedStyle(chat);
  return {
    prefix: fixture.dataset.prefix, prefixLength: Number(fixture.dataset.prefixLength),
    finalized: fixture.dataset.finalized === 'true',
    document: { clientWidth: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth },
    chat: { ...size(chat), containerType: style.containerType, contentWidth: chat.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight) },
    cases: Object.fromEntries([...document.querySelectorAll('[data-table-case]')].map(section => {
      const content = section.querySelector('.message-content');
      const prose = content.querySelector('p');
      return [section.dataset.tableCase, {
        row: size(section), content: size(content), prose: rect(prose),
        paragraphs: [...content.querySelectorAll('p')].map(node => ({ text: node.textContent, ...rect(node) })),
        markup: content.innerHTML,
        codeBlocks: [...content.querySelectorAll('code.language-text, pre code')].map(node => node.textContent.trim()),
        codeShells: [...content.querySelectorAll('pre')].map(node => ({ text: node.textContent, ...rect(node) })),
        headings: [...content.querySelectorAll('h1,h2,h3,h4,h5,h6')].map(node => node.textContent),
        tables: [...content.querySelectorAll('.markdown-table-scroll')].map(wrapper => {
          const table = wrapper.querySelector('table');
          const origin = rect(table);
          return {
            wrapper: size(wrapper), table: size(table), overflowX: getComputedStyle(wrapper).overflowX,
            beyondProse: rect(wrapper).left < rect(prose).left - 1 || rect(wrapper).right > rect(prose).right + 1,
            markup: wrapper.outerHTML,
            cells: [...table.querySelectorAll('th,td')].map(cell => {
              const r = rect(cell);
              return { text: cell.textContent, left: r.left - origin.left, top: r.top - origin.top, width: r.width, height: r.height };
            }),
          };
        }),
      }];
    })),
  };
}

function validate(sample, complete) {
  const label = `${sample.width}/${sample.prefix}/${sample.finalized ? 'final' : 'stream'}`;
  check(sample.chat.containerType === 'inline-size', `${label}: missing actual chat query container`);
  check(sample.document.scrollWidth <= sample.document.clientWidth + 1, `${label}: document horizontal overflow`);
  if (sample.width === 1280) {
    check(sample.chat.contentWidth > 800, `${label}: desktop container must exceed 800px`);
    for (const value of Object.values(sample.cases)) {
      check(near(value.row.width, 800), `${label}: desktop row not 800px`);
      check(near(value.row.left + value.row.width / 2, sample.chat.left + sample.chat.width / 2), `${label}: row not centered`);
    }
  }
  const { subject, reference } = sample.cases;
  for (const [name, value] of Object.entries(sample.cases)) {
    if (!baseline) {
      for (const key of ['row', 'content']) check(value[key].scrollWidth <= value[key].clientWidth + 1, `${label}/${name}: horizontal overflow escapes table into ${key}`);
    }
    for (const table of value.tables) {
      if (table.beyondProse) breakoutCount++;
      if (!baseline) check(!table.beyondProse, `${label}/${name}: table wrapper escapes prose`);
      check(['auto', 'scroll'].includes(table.overflowX), `${label}/${name}: table overflow is not local`);
      check(table.wrapper.left >= sample.chat.left - 1 && table.wrapper.right <= sample.chat.right + 1, `${label}/${name}: table escapes chat`);
    }
    if (complete) {
      check(value.tables.length === 1, `${label}/${name}: expected completed table`);
      check(JSON.stringify(value.tables.flatMap(table => table.cells.map(cell => cell.text))) === JSON.stringify(expectedCells), `${label}/${name}: table data lost or changed`);
    }
  }
  if (!complete) return;
  for (const key of ['codeBlocks', 'headings']) check(JSON.stringify(subject[key]) === JSON.stringify(reference[key]), `${label}: ${key} content differs`);
  check(subject.paragraphs.map(p => p.text).join('\n') === reference.paragraphs.map(p => p.text).join('\n'), `${label}: prose content differs`);
  check(near(subject.prose.left, reference.prose.left) && near(subject.prose.width, reference.prose.width), `${label}: prose bounds differ`);
  if (!baseline && subject.tables.length === 1 && reference.tables.length === 1) {
    const a = subject.tables[0];
    const b = reference.tables[0];
    check(near(a.wrapper.top - subject.content.top, b.wrapper.top - reference.content.top), `${label}: table position inside prose differs`);
    subject.paragraphs.forEach((paragraph, index) => {
      const other = reference.paragraphs[index];
      check(other && near(paragraph.top - subject.content.top, other.top - reference.content.top), `${label}: paragraph ${index} position differs`);
    });
    for (const key of ['left', 'width', 'height']) check(near(a.wrapper[key], b.wrapper[key]), `${label}: wrapper ${key} differs from finalized same-prefix reference`);
    check(a.markup === b.markup, `${label}: table markup differs from finalized reference`);
    a.cells.forEach((cell, index) => {
      for (const key of ['left', 'top', 'width', 'height']) check(near(cell[key], b.cells[index]?.[key] ?? NaN), `${label}: cell ${index} ${key} differs`);
    });
  }
}

const browser = await (engine === 'webkit' ? webkit : chromium).launch();
try {
  for (const width of [375, 800, 1280]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 }, deviceScaleFactor: 1 });
    page.on('pageerror', error => errors.push(`${width}: ${error.message}`));
    // Separate runs let both the completed-table/incomplete-tail and full-text
    // checkpoints undergo a real streaming -> finalized component replacement.
    for (const target of ['completed-table-incomplete-tail', 'full-text']) {
      const url = new URL(process.env.LADLE_URL);
      url.searchParams.set('story', 'streaming-table--full');
      url.searchParams.set('mode', 'preview');
      await page.goto(url.href);
      await page.waitForSelector('[data-table-ready="true"]');
      await page.evaluate(() => document.fonts.ready);
      while (true) {
        await page.waitForSelector('[data-table-ready="true"]');
        const sample = { width, target, ...await page.evaluate(measure) };
        samples.push(sample);
        validate(sample, ['completed-table-incomplete-tail', 'full-text'].includes(sample.prefix));
        if (sample.prefix === target) break;
        await page.getByTestId('next-prefix').click();
      }
      const stem = `${width}-${target}`;
      await page.screenshot({ path: path.join(out, `${stem}-before.png`), fullPage: true });
      await page.locator('[data-table-case="subject"] .markdown-table-scroll').screenshot({ path: path.join(out, `${stem}-table-before.png`) });
      const before = samples.at(-1);
      await page.getByTestId('finalize-table').click();
      await page.waitForSelector('[data-table-ready="true"][data-finalized="true"]');
      const after = { width, target, ...await page.evaluate(measure) };
      samples.push(after);
      validate(after, true);
      check(after.prefixLength === before.prefixLength, `${stem}: finalization changed prefix`);
      if (target === 'full-text') check(after.prefixLength === source.length, `${stem}: full reply was truncated`);
      else check(after.prefixLength < source.length, `${stem}: tail must remain incomplete`);
      if (!baseline) {
        const a = before.cases.subject.tables[0]?.wrapper;
        const b = after.cases.subject.tables[0]?.wrapper;
        for (const key of ['left', 'top', 'width', 'height']) check(a && b && near(a[key], b[key]), `${stem}: ${key} changed during finalization`);
      }
      await page.screenshot({ path: path.join(out, `${stem}-after.png`), fullPage: true });
      await page.locator('[data-table-case="subject"] .markdown-table-scroll').screenshot({ path: path.join(out, `${stem}-table-after.png`) });
    }
    await page.close();
  }
  if (baseline) check(breakoutCount > 0, 'Baseline must reproduce at least one table wrapper beyond prose bounds');
  check(errors.length === 0, `Browser errors: ${errors.join('; ')}`);
} catch (error) {
  failures.push(error.stack ?? String(error));
} finally {
  await writeFile(path.join(out, 'metrics.json'), JSON.stringify({ baseline, engine, breakoutCount, failures, errors, samples }, null, 2));
  await browser.close();
}
console.log(`${samples.length} samples; ${breakoutCount} breakouts; artifacts: ${out}`);
assert.equal(failures.length, 0, failures.join('\n'));
