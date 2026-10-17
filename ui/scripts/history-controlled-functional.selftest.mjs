import assert from 'node:assert/strict';
import test from 'node:test';
import { EventEmitter } from 'node:events';
import { assertControlledInventory, assertOwnedReport, runControlledFunctional, settleControlledHistoricalTarget, CONTROLLED_VIEWPORT } from './history-controlled-functional.mjs';
import { verifyServedArtifact, captureOwnedBrowser, resolveOwnedChromium } from './qualify-history-controlled-functional.mjs';
import { createHash } from 'node:crypto';
const LABEL = 'UNIT_ONLY_NOT_RUNTIME_PROOF';
function inventory() {
  const trials = ['history', 'pagehide', 'cancellation'].flatMap(kind => ['ordinary','global'].map(route => {
    const open_id = `UNIT-${route}-${kind}`;
    return { route, kind, status: 'passed_assertions', admission: { sampled:true, open_id, route },
      reports:[{open_id,route,sample_rate:1,outcome:kind==='history'?'ready':kind==='pagehide'?'incomplete':'canceled'}], journeys:[{open_id}],
      primaryRequestOpenIds:['UNIT-request'], statuses:kind==='pagehide'?[]:[204], errors:[], delivery_proof:'provisional_until_real_collector_and_export_join' };
  }));
  return { evidence_class:'controlled_functional', overhead_clearance:false,full_acceptance:false,resume:{status:'unsupported'},trials,journeys:trials.flatMap(t=>t.journeys) };
}
test('UNIT six-report controlled inventory cannot become stock, overhead or resume acceptance', () => {
  assertControlledInventory(inventory());
  for (const change of [r=>r.trials.pop(),r=>r.trials.push(r.trials[0]),r=>r.journeys.pop(),r=>r.evidence_class='stock_natural',r=>r.overhead_clearance=true,r=>r.full_acceptance=true,r=>r.resume.status='passed',r=>r.trials[0].reports[0].sample_rate=.25,r=>r.trials[0].reports[0].outcome='incomplete',r=>r.trials[2].delivery_proof='synthetic',r=>r.trials[4].statuses=[],r=>r.trials[1].reports.push(r.trials[1].reports[0]),r=>r.trials[3].primaryRequestOpenIds=[],r=>r.trials[0].reports[0].route='global',r=>r.trials[0].admission.route='global']) {
    const r=inventory();change(r);assert.throws(()=>assertControlledInventory(r));
  }
});
test('UNIT controlled correlation requires independent same-journey producer and finite network provenance',()=>{
  const report={open_id:'UNIT-journey',correlations:[{kind:'request',id:'UNIT-request'},{kind:'sse',id:'UNIT-stream'},{kind:'trace',id:'UNIT-trace'}]};
  const data={ownershipFailure:null,errors:[],requestOwners:[{journey_id:report.open_id,request_id:'UNIT-request'}],streamOwners:[{journey_id:report.open_id,stream_id:'UNIT-stream'}],requestRecords:[{kind:'request',method:'GET',open_id:'UNIT-request',trace_id:'UNIT-trace'},{kind:'sse',open_id:'UNIT-stream'}]};
  assertOwnedReport(report,data);
  for(const change of [d=>d.requestOwners[0].journey_id='foreign',d=>d.streamOwners[0].journey_id='foreign',d=>d.requestRecords.pop(),d=>d.requestRecords[0].trace_id='foreign',d=>d.requestRecords[0].method='POST',d=>d.ownershipFailure='invalid']){
    const d=structuredClone(data);change(d);assert.throws(()=>assertOwnedReport(report,d));
  }
});
test('UNIT independently served controlled bytes must match every admitted asset',async()=>{
  const bytes=Buffer.from('UNIT-ASSET'), hash=createHash('sha256').update(bytes).digest('hex');
  const receipt={evidence_class:'controlled_functional',sample_rate:1,ui_files:{'index.html':hash}};
  const fake=async()=>({status:200,arrayBuffer:async()=>bytes});
  await verifyServedArtifact('http://127.0.0.1:12345',receipt,fake);
  for(const r of [{...receipt,evidence_class:'stock_natural'},{...receipt,sample_rate:.25},{...receipt,ui_files:{'../escape':hash}},{...receipt,ui_files:{'index.html':'0'.repeat(64)}}]) await assert.rejects(verifyServedArtifact('http://127.0.0.1:12345',r,fake));
  await assert.rejects(verifyServedArtifact('http://127.0.0.1:12345',receipt,async()=>({status:302})));
});
function fakeRuntime(options={}){
  let count=0,closed=0;const pages=[];
  const fixtures={identity:{serverInstanceId:'UNIT-instance'},aggregates:['ordinary','global'].map(kind=>({kind,route:`/${kind}/latest`,endpoint:`/api/${kind}/history`,product_conversation_id:`UNIT-${kind}`}))};
  const targets=fixtures.aggregates.map(f=>({kind:f.kind,route:`/${f.kind}/root`,anchor:{sourceRow:'UNIT-root',persistedMessageID:'UNIT-original'},composerOwner:'UNIT-latest'}));
  class Context {
    async exposeBinding(_,callback){this.callback=callback;}
    async addInitScript(){}
    async newCDPSession(){return {send:async()=>{}};}
    async newPage(){const p=new Page(this);pages.push(p);return p;}
    async close(){if(!this.closed){
      assert.ok(!this.page?.reportInFlight, 'context must not close before departing transport settles');
      this.closed=true;closed++;if(options.lateReport&&this.page)this.page.report('ready');
      if(options.closeFailure)throw new Error('UNIT private cleanup detail');
    }}
  }
  class Page extends EventEmitter {
    constructor(context){super();this.context=context;context.page=this;this.frame={};this.open_id=`UNIT-open-${++count}`;}
    mainFrame(){return this.frame;}
    async close(){}
    url(){return this.location;}
    diagnostic(event){this.context.callback({page:this,frame:this.frame},event);}
    request(path,method='GET',body){const req={url:()=>new URL(path,'http://127.0.0.1:12345').href,method:()=>method,postData:()=>body};this.emit('request',req);return req;}
    async goto(url){
      this.location=url;
      if(url==='about:blank'){this.report('incomplete');return;}
      this.fixture=fixtures.aggregates.find(f=>new URL(url).pathname.startsWith('/'+f.kind));
      this.request_id='UNIT-request-'+this.open_id;
      this.diagnostic({kind:'admission',open_id:this.open_id,sampled:true,route:this.fixture.kind});
      this.diagnostic({kind:'request',open_id:this.open_id,request_id:this.request_id});
      this.pending=this.request(this.fixture.endpoint+'?open_id='+this.request_id);
      if(url.endsWith('/root')){
        if(!options.omitHistoryRequest){
          this.diagnostic({kind:'request',open_id:this.open_id,request_id:this.request_id+'-older'});
          this.request(this.fixture.endpoint+'?before=UNIT-cursor&open_id='+this.request_id+'-older');
        }
        this.diagnostic({kind:'sse',open_id:this.open_id,stream_id:'UNIT-stream'});
        this.request('/api/conversations/UNIT-latest/stream?open_id=UNIT-stream');
        this.report('ready');
      }
    }
    report(outcome){
      const report={open_id:this.open_id,route:options.wrongRoute?'foreign':this.fixture.kind,sample_rate:1,outcome,product_conversation_id:this.fixture.product_conversation_id,correlations:[{kind:'request',id:this.request_id}]};
      if(outcome==='ready'&&!options.omitHistoryRequest&&!options.omitHistoryCorrelation)report.correlations.push({kind:'request',id:this.request_id+'-older'});
      const req=this.request('/api/telemetry/conversation-view','POST',JSON.stringify(report));
      if(outcome!=='incomplete')this.emit('response',{request:()=>req,status:()=>204});
      if(outcome==='incomplete'&&options.delayedDeparture){
        this.reportInFlight=true;
        setImmediate(()=>{this.reportInFlight=false;this.emit('requestfinished',req);});
      }else this.emit('requestfinished',req);
    }
    async evaluate(_fn,arg){
      if(options.completeBeforeAction&&this.pending)this.emit('requestfinished',this.pending);
      if(!arg){await new Promise(resolve=>setImmediate(resolve));return 123;}
      this.physicalReads=(this.physicalReads??0)+1;
      const missing=options.missingPhysical||(options.delayedPhysical&&this.physicalReads===1);
      return {observation:missing?null:{...arg.known[0],key:'UNIT-virtual',offset:0,messageOffset:0,currentURL:this.location,
        sourcepin:options.wrongSourcepin?'UNIT-foreign':arg.known[0].sourceRow},predicates:missing?{known_target_row_clipped:1}:{}};
    }
    locator(selector){
      assert.ok(!selector.includes('placeholder'), 'composer identity cannot depend on dynamic presentation text');
      if(selector.startsWith('textarea'))assert.equal(selector,'textarea#message-input');
      return {count:async()=>options.missingComposer?0:1,isEditable:async()=>!options.readonlyComposer,waitFor:async()=>{},isEnabled:async()=>true,evaluate:async()=> 'BUTTON',click:async()=>{this.location='http://127.0.0.1:12345/';this.report('canceled');}};}
    async waitForURL(){}
  }
  const contextOptions=[];
  const browser={newContext:async settings=>{contextOptions.push(settings);return new Context();}};
  const module={captureReport:raw=>JSON.parse(raw),journeyExpectation:r=>({open_id:r.open_id}),sidebarHomeSelector:'UNIT-home'};
  const UNIT={label:LABEL,authenticate:async()=>{},ownership:{label:'UNIT_ONLY_NOT_FIXTURE_PROOF',module:{captureDiagnostic:raw=>raw}}};
  return {input:{browser,baseURL:'http://127.0.0.1:12345',fixtures,module,historyTargets:targets,config:{unitOnly:true,evidence_class:'controlled_functional'}},UNIT,getClosed:()=>closed,pages,contextOptions};
}
test('UNIT full six-report driver traverses real action interfaces and closes all contexts without stock cohorts',async()=>{
  const runtime=fakeRuntime();const result=await runControlledFunctional(runtime.input,runtime.UNIT);
  assertControlledInventory(result);assert.equal(runtime.getClosed(),6);assert.equal(runtime.pages.length,6);
});
test('UNIT late context-close report is retained and fails exact inventory',async()=>{
  const runtime=fakeRuntime({lateReport:true});await assert.rejects(runControlledFunctional(runtime.input,runtime.UNIT));assert.equal(runtime.getClosed(),1);
});
test('UNIT controlled driver refuses injected runtime proof',async()=>{
  const runtime=fakeRuntime();runtime.input.config.unitOnly=false;
  await assert.rejects(runControlledFunctional(runtime.input,runtime.UNIT),/cannot become runtime proof/);
});

test('UNIT request completing between pending observation and actual departure cannot qualify',async()=>{
  const runtime=fakeRuntime({completeBeforeAction:true});
  await assert.rejects(runControlledFunctional(runtime.input,runtime.UNIT),/remains pending/);
  assert.equal(runtime.getClosed(),3);
});
test('UNIT CLI uses the actual preparation-helper object API and keeps controlled output separate',async()=>{
  const {readFileSync}=await import('node:fs');
  const source=readFileSync(new URL('./qualify-history-controlled-functional.mjs',import.meta.url),'utf8');
  assert.match(source,/prepareJointTelemetry\(\{ env: process\.env, appRoot: app \}\)/);
  assert.match(source,/config\.input\.fixtures/);
  assert.match(source,/HISTORY_OUTPUT, 'browser-result\.json'/);
  assert.doesNotMatch(source,/runQualification\(/);
});

test('UNIT delayed departing transport settles before context destruction without claiming delivery',async()=>{
  const runtime=fakeRuntime({delayedDeparture:true});
  const result=await runControlledFunctional(runtime.input,runtime.UNIT);
  for(const t of result.trials.filter(t=>t.kind==='pagehide'))assert.equal(t.transport_settled_before_context_close,true);
  assert.equal(runtime.getClosed(),6);
});
test('UNIT wrong actual report route is rejected independently of producer route',async()=>{
  const runtime=fakeRuntime({wrongRoute:true});
  await assert.rejects(runControlledFunctional(runtime.input,runtime.UNIT),/report route/);
  assert.equal(runtime.getClosed(),1);
});
test('UNIT primary assertion survives cleanup failure and retains failing collected inventory',async()=>{
  const runtime=fakeRuntime({lateReport:true,closeFailure:true});
  let error;try{await runControlledFunctional(runtime.input,runtime.UNIT);}catch(e){error=e;}
  assert.ok(error);assert.equal(error.controlledReceipt.failed_trial.reports.length,2);
});
test('UNIT browser lifecycle preserves six-report receipt when auth-negative or final cleanup fails',async()=>{
  for(const failure of ['negative','close']){
    const published=[];const original=inventory();
    await assert.rejects(captureOwnedBrowser({launch:async()=>({close:async()=>{if(failure==='close')throw new Error('UNIT SECRET');}}),
      capture:async()=>original,negative:async()=>{if(failure==='negative')throw new Error('UNIT SECRET');},
      publish:r=>published.push(structuredClone(r)),timeoutMs:100,closeMs:10}));
    const last=published.at(-1);assert.equal(last.status,'failed');assert.equal(last.trials.length,6);
    assert.ok(!JSON.stringify(published).includes('SECRET'));
  }
});
test('UNIT stalled launch and stalled close publish bounded failure receipts',async()=>{
  for(const failure of ['launch','close']){
    const published=[];const stall=()=>new Promise(()=>{});
    await assert.rejects(captureOwnedBrowser({launch:failure==='launch'?stall:async()=>({close:stall}),
      capture:async()=>inventory(),negative:async()=>{},publish:r=>published.push(structuredClone(r)),timeoutMs:5,closeMs:5}));
    assert.equal(published.at(-1).status,'failed');
  }
});

test('UNIT stalled setup hits the absolute deadline and late completion cannot create a page',async()=>{
  const runtime=fakeRuntime();let finish;
  runtime.UNIT.authenticate=()=>new Promise(resolve=>{finish=resolve;});
  runtime.input.deadlineMs=5;
  await assert.rejects(runControlledFunctional(runtime.input,runtime.UNIT),/deadline/);
  assert.equal(runtime.getClosed(),1);assert.equal(runtime.pages.length,0);
  finish();await new Promise(resolve=>setImmediate(resolve));
  assert.equal(runtime.pages.length,0);
});
test('UNIT a primary setup failure is not masked by a context cleanup failure',async()=>{
  const runtime=fakeRuntime({closeFailure:true});const primary=new Error('UNIT primary setup');
  runtime.UNIT.authenticate=async()=>{throw primary;};
  let error;try{await runControlledFunctional(runtime.input,runtime.UNIT);}catch(e){error=e;}
  assert.equal(error,primary);assert.deepEqual(error.controlledReceipt.cleanup_failures,['controlled_context_close_failed']);
});

test('UNIT historical ready cannot qualify without actual additional history and its original-owner correlation',async()=>{
  for(const options of [{omitHistoryRequest:true},{omitHistoryCorrelation:true}]){
    const runtime=fakeRuntime(options);
    await assert.rejects(runControlledFunctional(runtime.input,runtime.UNIT),/additional history/);
  }
});

test('offline declared Playwright dependency resolves through the actual pinned app without browser launch',async()=>{
  const {resolve}=await import('node:path');const {readFileSync}=await import('node:fs');
  const app=resolve(process.env.APP_ROOT??'target/history-hidden-geometry-repair');
  const pkg=JSON.parse(readFileSync(resolve(app,'ui/package.json'),'utf8'));
  assert.ok(pkg.devDependencies.playwright);assert.equal(pkg.devDependencies['@playwright/test'],undefined);
  const chromium=resolveOwnedChromium(app);assert.equal(typeof chromium.launch,'function');
});

test('UNIT outer lifecycle preserves allowlisted scenario failure location instead of erasing it',async()=>{
  const runtime=fakeRuntime({omitHistoryRequest:true});const published=[];
  await assert.rejects(captureOwnedBrowser({launch:async()=>runtime.input.browser,
    capture:()=>runControlledFunctional(runtime.input,runtime.UNIT),negative:async()=>{},
    publish:r=>published.push(structuredClone(r)),timeoutMs:1000}));
  const result=published.at(-1);assert.equal(result.failure,'controlled_scenario_failed');
  assert.deepEqual(result.failure_location,{scope:'scenario',route:'ordinary',kind:'history',
    phase:'additional_history_request_present',error_kind:'assertion',counts:{reports:1,responses:[204],admissions:1,requests:2,pending:1,errors:0},physical:{predicates:{},observation_present:true,fixture_identity_matches:true,sourcepin_matches:true},pagination:{errors:0,requests:[]}});
});
test('UNIT phase receipts expose bounded counts, not raw errors or private setup details',async()=>{
  const runtime=fakeRuntime();const published=[];
  runtime.UNIT.authenticate=async()=>{throw new Error('SECRET credential path and body');};
  await assert.rejects(captureOwnedBrowser({launch:async()=>runtime.input.browser,
    capture:()=>runControlledFunctional(runtime.input,runtime.UNIT),negative:async()=>{},publish:r=>published.push(structuredClone(r))}));
  assert.equal(published.at(-1).failure_location.phase,'authenticate');
  assert.ok(!JSON.stringify(published).includes('SECRET'));
});

test('UNIT physical failure distinguishes strict oracle null from source-pin mismatch without leaking identity',async()=>{
  for(const [options,phase,present,pin] of [
    [{missingPhysical:true},'physical_history_present',false,null],
    [{wrongSourcepin:true},'physical_history_sourcepin',true,false],
  ]){
    const runtime=fakeRuntime(options);let error;
    if(options.missingPhysical)runtime.input.deadlineMs=10;
    try{await runControlledFunctional(runtime.input,runtime.UNIT);}catch(e){error=e;}
    assert.ok(error);const location=error.controlledReceipt.failure_location;
    assert.equal(location.phase,phase);assert.equal(location.physical.observation_present,present);
    assert.equal(location.physical.sourcepin_matches,pin);
    assert.equal(location.physical.fixture_identity_matches,present?true:null);
    assert.ok(!JSON.stringify(location).includes('UNIT-foreign'));
    assert.ok(!JSON.stringify(location).includes('UNIT-row'));
    assert.equal(runtime.getClosed(),1);
  }
});

test('UNIT explicit qualification viewport and physical settlement are independent of first report204',async()=>{
  const runtime=fakeRuntime({delayedPhysical:true});
  const result=await runControlledFunctional(runtime.input,runtime.UNIT);
  assert.deepEqual(runtime.contextOptions,Array.from({length:6},()=>({viewport:{width:1360,height:900}})));
  assert.deepEqual(CONTROLLED_VIEWPORT,{width:1360,height:900});
  for(const trial of result.trials.filter(t=>t.kind==='history')){
    assert.equal(trial.physical_history.stability_frames,4);
    assert.equal(trial.physical_history.anchor.stability_frames.length,4);
  }
  assert.equal(runtime.pages[0].physicalReads,6,'null after204 then strict acquisition then four frames');
});
test('UNIT physical settlement is bounded and cannot accept permanent clipping or substitute unstable identity',async()=>{
  let remaining=4,frames=0;
  await assert.rejects(settleControlledHistoricalTarget({read:async()=>null,nextFrame:async()=>{frames++;},
    check:()=>assert.ok(remaining-->0,'UNIT budget exhausted')}),/budget/);
  assert.ok(frames>0);
  const first={key:'UNIT-key',persistedMessageID:'UNIT-id',sourceRow:'UNIT-source',sourcepin:'UNIT-source',currentURL:'UNIT-url',offset:0,messageOffset:0};
  for(const change of [{offset:3},{persistedMessageID:'UNIT-other'},{sourcepin:'UNIT-other'},{currentURL:'UNIT-other'}]){
    let reads=0;
    await assert.rejects(settleControlledHistoricalTarget({read:async()=>++reads===1?first:{...first,...change},nextFrame:async()=>{},check:()=>{}}),/four frames/);
    assert.equal(reads,2);
  }
});

test('UNIT dynamic-placeholder-independent composer still requires exactly one editable canonical input',async()=>{
  for(const options of [{missingComposer:true},{readonlyComposer:true}]){
    const runtime=fakeRuntime(options);let error;
    try{await runControlledFunctional(runtime.input,runtime.UNIT);}catch(e){error=e;}
    assert.ok(error);assert.equal(error.controlledReceipt.failure_location.phase,'composer_owner');
    assert.equal(runtime.getClosed(),1);
  }
});
