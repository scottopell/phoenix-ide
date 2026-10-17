import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { readFileSync, writeFileSync } from 'node:fs';
import { authenticateOwnedContext } from './history-joint-telemetry.mjs';
import { runBFCacheRoundTrip, captureReport } from './qualify-view-telemetry.mjs';
import { observePhysicalHistoricalRow } from './history-postfault-baseline.mjs';

const input=JSON.parse(readFileSync(process.env.PROBE_INPUT,'utf8'));
const out=resolve(process.env.PROBE_OUTPUT,'native-capability.json');
const base=process.env.HISTORY_BASE_URL;
const {chromium}=createRequire(resolve(process.env.APP_ROOT,'ui/package.json'))('playwright');
const result={scope:'separate_desktop_native_bfcache_probe_controlled_artifact_not_PWA_or_physical_suspend',
  full_acceptance:false,overhead_clearance:false,status:'failed',reports:[],rejections:[],
  launch:{headless:true,ignoreDefaultArgs:['--disable-back-forward-cache']}};
let browser,context,page,parseErrors=0;
const save=()=>writeFileSync(out,JSON.stringify(result,null,2));
const watchdog=setTimeout(()=>{result.failure='absolute_probe_deadline';save();void browser?.close();},65000);
const physical=async()=>page.evaluate(({oracle,input})=>(0,eval)('('+oracle+')')({known:[{persistedMessageID:input.id,sourceRow:input.source}]}),{oracle:observePhysicalHistoricalRow.toString(),input});
try {
  browser=await chromium.launch({...result.launch,timeout:10000});
  result.browser_version=browser.version();
  const session=await browser.newBrowserCDPSession();
  const command=await session.send('Browser.getBrowserCommandLine');
  result.effective_bfcache_disable_present=command.arguments.includes('--disable-back-forward-cache');
  result.effective_launch_switches=command.arguments.filter(arg=>arg.startsWith('--')&&!arg.startsWith('--user-data-dir'));
  assert.equal(result.effective_bfcache_disable_present,false);
  await session.detach();
  context=await browser.newContext({viewport:{width:1360,height:900}});
  await authenticateOwnedContext(context,base);
  page=await context.newPage();
  const diagnostics=await context.newCDPSession(page);
  await diagnostics.send('Page.enable');
  diagnostics.on('Page.backForwardCacheNotUsed',event=>{
    result.rejections.push({reasons:event.notRestoredExplanations?.slice(0,32).map(e=>({type:e.type,reason:e.reason}))??[]});
    if(result.rejections.length>4) result.rejections.shift();
  });
  const accepted=new Set();
  page.on('request',request=>{
    if(new URL(request.url()).pathname==='/api/telemetry/conversation-view'&&request.method()==='POST'){
      try {assert.ok(result.reports.length<4);result.reports.push(captureReport(request.postData()??''));}
      catch {parseErrors++;}
    }
  });
  page.on('response',response=>{
    if(new URL(response.url()).pathname==='/api/telemetry/conversation-view'&&response.status()===204){
      try {accepted.add(JSON.parse(response.request().postData()).open_id);}catch {parseErrors++;}
    }
  });
  await page.goto(base+input.route,{waitUntil:'domcontentloaded',timeout:20000});
  await page.waitForFunction(()=>document.visibilityState==='visible'&&document.querySelector('#messages')&&document.querySelector('textarea#message-input'),undefined,{timeout:15000});
  const initialDeadline=Date.now()+15000;
  while(!result.reports.some(r=>r.outcome==='ready'&&accepted.has(r.open_id))&&Date.now()<initialDeadline)
    await page.evaluate(()=>new Promise(r=>requestAnimationFrame(r)));
  const initial=result.reports.find(r=>r.outcome==='ready'&&accepted.has(r.open_id));
  assert.ok(initial,'initial real accepted ready report required');
  result.initial_anchor=await physical();assert.ok(result.initial_anchor);
  assert.equal(initial.standalone,false);
  assert.equal(initial.sample_rate,1);
  result.initial_report_id=initial.open_id;
  result.witness=await runBFCacheRoundTrip(page,base);save();
  if(result.witness.status==='actual_bfcache_restore_observed'){
    const end=Date.now()+10000;
    while(!result.reports.some(r=>r.open_id!==initial.open_id&&r.trigger==='back_forward'&&accepted.has(r.open_id))&&Date.now()<end)
      await page.evaluate(()=>new Promise(r=>requestAnimationFrame(r)));
    const resumed=result.reports.find(r=>r.open_id!==initial.open_id&&r.trigger==='back_forward'&&accepted.has(r.open_id));
    result.resumed_anchor=await physical();
    if(resumed){
      assert.equal(resumed.product_conversation_id,initial.product_conversation_id);
      assert.equal(resumed.route,initial.route);assert.equal(resumed.cache,'retained');
      assert.equal(resumed.outcome,'ready');assert.equal(resumed.visible,true);
      for(const key of ['meaningful_transcript','breadcrumb_presented','breadcrumb_enabled','anchor_settled'])assert.ok(Number.isFinite(resumed.marks[key]));
      assert.ok(result.resumed_anchor,'fresh resumed physical owning row required');
      result.status='native_restore_and_fresh_ready_anchor_observed';
    }else result.status='native_restore_observed_fresh_ready_report_missing';
  } else result.status='unsupported_no_actual_bfcache_restore';
  result.accepted_report_ids=[...accepted];
  assert.equal(parseErrors,0);
} catch {
  result.failure='bounded_probe_prerequisite_or_assertion_failed';process.exitCode=1;
} finally {
  result.capture_errors=parseErrors;
  try {await context?.close();await browser?.close();result.browser_closed=true;}
  catch {result.cleanup_failure=true;process.exitCode=1;}
  clearTimeout(watchdog);save();
}
