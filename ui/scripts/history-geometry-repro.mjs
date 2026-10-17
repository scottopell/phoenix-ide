import {createRequire} from 'node:module';
import {resolve} from 'node:path';
import {readFileSync,writeFileSync} from 'node:fs';
import {authenticateOwnedContext,installHistoryPaginationDiagnostic} from './history-joint-telemetry.mjs';
import {observePhysicalHistoricalRow} from './history-postfault-baseline.mjs';
export function geometry({id, source, route}) {
  const rect=e=>{if(!e)return null;const b=e.getBoundingClientRect();return Object.fromEntries(['top','bottom','left','right','width','height'].map(k=>[k,Number.isFinite(b[k])?b[k]:null]));};
  const scroller=document.querySelector('#messages'),sr=rect(scroller);
  const matches=[...document.querySelectorAll('[id], [data-message-id]')].filter(e=>e.id==='message-'+id||e.getAttribute('data-message-id')===id);
  const child=matches.length===1?matches[0]:null,row=child?.closest('[data-virtual-key]');
  const clip=sr?{top:Math.max(0,sr.top),bottom:Math.min(innerHeight,sr.bottom),left:Math.max(0,sr.left),right:Math.min(innerWidth,sr.right)}:null;
  const inspect=e=>{if(!e)return null;const b=rect(e),s=getComputedStyle(e);return {rect:b,edges:clip?{top:b.top-clip.top,bottom:clip.bottom-b.bottom,left:b.left-clip.left,right:clip.right-b.right}:null,
    clientRects:e.getClientRects().length,connected:e.isConnected,styles:Object.fromEntries(['display','visibility','overflowX','overflowY','contain','position','transform','boxSizing','borderTopWidth','borderBottomWidth','borderLeftWidth','borderRightWidth','paddingTop','paddingBottom','paddingLeft','paddingRight'].map(k=>[k,s[k]]))};};
  const composers=[...document.querySelectorAll('textarea#message-input')];
  const composer={count:composers.length,states:composers.slice(0,4).map(e=>({disabled:e.disabled,readOnly:e.readOnly,connected:e.isConnected,visible:e.getClientRects().length>0,rect:rect(e)}))};
  const ancestors=[];for(let e=child?.parentElement;e&&ancestors.length<8;e=e.parentElement)ancestors.push(inspect(e));
  return {viewport:{innerWidth,innerHeight,devicePixelRatio,visualViewport:visualViewport?{width:visualViewport.width,height:visualViewport.height,offsetTop:visualViewport.offsetTop,offsetLeft:visualViewport.offsetLeft,scale:visualViewport.scale}:null},
    document:rect(document.documentElement),scroller:inspect(scroller),scroll:scroller?Object.fromEntries(['clientTop','clientLeft','clientHeight','clientWidth','scrollTop','scrollLeft','scrollHeight','scrollWidth'].map(k=>[k,scroller[k]])):null,
    composer,clip,child:inspect(child),row:inspect(row),inner:inspect(row?.querySelector('.virtual-transcript-row')),header:inspect(document.querySelector('.virtual-transcript__header')),ancestors,
    identity:{physical_matches:matches.length,fixture_id_matches:!!child,alias_agrees:!child?.getAttribute('data-message-id')||child.getAttribute('data-message-id')===id,
      sourcepin_matches:new URL(location.href).searchParams.get('source_transcript')===source,original_route:location.pathname+location.search+location.hash===route}};
}
export function installWriteCapture({input, geometrySource}) {
  const probe=(0,eval)('('+geometrySource+')');
  const observe=input=>{try{return probe(input);}catch{return {instrumentation_geometry_failed:true};}};
  const events=[];let dropped=0,sequence=0,installed=false,failures=0;
  const record=event=>{try{if(events.length<128)events.push({sequence:++sequence,...event});else dropped++;}catch{dropped++;}};
  const site=()=>{
    const raw=String(new Error().stack??'');
    const owner=['applyAlignedPosition','applyAnchor','resolvePendingTarget','positionAtIndex','writeDomBottom','reconcileDrift','scrollIntoView'].find(n=>raw.includes(n))??'compiled_unresolved';
    const calls=[...raw.matchAll(/\/assets\/([A-Za-z0-9_-]+\.js):(\d+):(\d+)/g)].slice(0,5).map(m=>({asset:m[1],line:Number(m[2]),column:Number(m[3])}));
    return {owner,calls};
  };
  const safeSite=()=>{try{return site();}catch{failures++;return {owner:'instrumentation_failed',calls:[]};}};
  const find=(prototype,name)=>{for(let p=prototype;p;p=Object.getPrototypeOf(p)){const descriptor=Object.getOwnPropertyDescriptor(p,name);if(descriptor)return {prototype:p,descriptor};}return null;};
  const found=find(Element.prototype,'scrollTop');
  if(found?.descriptor.get&&found?.descriptor.set){
    const {get,set}=found.descriptor;
    Object.defineProperty(found.prototype,'scrollTop',{...found.descriptor,set(value){
      if(this.id!=='messages'){set.call(this,value);return;}
      const before=get.call(this);set.call(this,value);const applied=get.call(this),callsite=safeSite();
      record({kind:'scrollTop_assignment',requested:Number.isFinite(value)?value:null,before,applied,callsite,geometry:observe(input)});
    }});installed=true;
  }else record({kind:'instrumentation_missing_scrollTop_descriptor'});
  for(const name of ['scrollTo','scrollBy','scrollIntoView']){
    const method=Element.prototype[name];if(typeof method!=='function')continue;
    Element.prototype[name]=function(...args){const result=Reflect.apply(method,this,args);
      try{const relevant=this.id==='messages'||this.closest?.('#messages');if(relevant)record({kind:name,callsite:safeSite(),geometry:observe(input)});}catch{failures++;}
      return result;};
  }
  const Native=window.ResizeObserver;
  window.ResizeObserver=class extends Native {
    constructor(callback){super((entries,observer)=>{
      try{for(const entry of entries){const el=entry.target;
        const tag=el.id==='messages'?'scroller':el.classList?.contains('virtual-transcript__header')?'header':null;
        if(tag)record({kind:'resize_observer',tag,content:{height:entry.contentRect.height,width:entry.contentRect.width},geometry:observe(input)});
      }}catch{failures++;}
      Reflect.apply(callback,observer,[entries,observer]);
    });}
  };
  Object.defineProperty(window,'__historyGeometryRead',{value:()=>({events:events.slice(),dropped,installed,failures}),configurable:false});
}
async function main(){
  const input=JSON.parse(readFileSync(process.env.GEOMETRY_INPUT,'utf8')),out=process.env.GEOMETRY_OUTPUT;
  const {chromium}=createRequire(resolve(process.env.APP_ROOT,'ui/package.json'))('playwright');
  const receipt={scope:'diagnostic_same_artifact_new_fixture_recipe_not_original_db',acceptance:false,observations:[],status:'failed'};
  let browser,context,page;const save=()=>writeFileSync(resolve(out,'geometry.json'),JSON.stringify(receipt,null,2));
  const watchdog=setTimeout(()=>{save();void browser?.close();},55000);
  try{
    browser=await chromium.launch({headless:true,timeout:10000});receipt.browser_version=browser.version();
    context=await browser.newContext({viewport:{width:1360,height:900}});await authenticateOwnedContext(context,process.env.HISTORY_BASE_URL);
    await context.addInitScript(installWriteCapture,{input,geometrySource:geometry.toString()});
    page=await context.newPage();let reports=0,reportCaptureErrors=0;const reportBodies=[];
    const pagination=installHistoryPaginationDiagnostic(page,process.env.HISTORY_BASE_URL,input.endpoint);
    page.on('request',r=>{if(new URL(r.url()).pathname==='/api/telemetry/conversation-view'&&r.method()==='POST'){
      try{const body=r.postData()??'';if(Buffer.byteLength(body)>8192||reportBodies.length>=7)throw new Error();reportBodies.push(JSON.parse(body));}
      catch{reportCaptureErrors=Math.min(128,reportCaptureErrors+1);}
    }});
    page.on('response',r=>{if(new URL(r.url()).pathname==='/api/telemetry/conversation-view'&&r.status()===204)reports++;});
    await page.goto(process.env.HISTORY_BASE_URL+input.route,{waitUntil:'domcontentloaded',timeout:20000});
    const deadline=Date.now()+20000;
    while(reports===0&&Date.now()<deadline)await page.evaluate(()=>new Promise(r=>requestAnimationFrame(r)));
    receipt.report204_count=reports;receipt.report_capture_errors=reportCaptureErrors;receipt.pagination=pagination.snapshot(reportBodies);
    for(let frame=0;frame<5;frame++){
      if(frame)await page.evaluate(()=>new Promise(r=>requestAnimationFrame(r)));
      const observation=await page.evaluate(async ({geometrySource,oracleSource,input})=>{
        const data=(0,eval)('('+geometrySource+')')(input);
        const physical=(0,eval)('('+oracleSource+')')({known:[{persistedMessageID:input.id,sourceRow:input.source}]});
        return {...data,strict_containment:physical!==null};
      },{geometrySource:geometry.toString(),oracleSource:observePhysicalHistoricalRow.toString(),input});
      receipt.observations.push(observation);receipt.write_capture=await page.evaluate(()=>window.__historyGeometryRead());save();
    }
    await page.locator('#messages').screenshot({path:resolve(out,'messages.png'),timeout:5000});receipt.status=reports>0&&reportCaptureErrors===0&&receipt.pagination.errors===0&&receipt.observations.length===5&&receipt.observations.every(o=>o.identity.fixture_id_matches)
      &&receipt.write_capture?.installed&&receipt.write_capture.failures===0&&receipt.write_capture.dropped===0
      &&receipt.write_capture.events.some(e=>e.kind==='scrollTop_assignment'&&!e.geometry.instrumentation_geometry_failed)?'captured':'capture_prerequisites_missing';save();
  }catch{receipt.failure='bounded_geometry_operation_failed';try{if(page)receipt.write_capture=await page.evaluate(()=>window.__historyGeometryRead());}catch{}save();}
  finally{try{await context?.close();await browser?.close();}catch{receipt.cleanup_failure=true;}clearTimeout(watchdog);save();}
  if(receipt.status!=='captured')process.exitCode=1;
}
if(process.argv[1]&&import.meta.url==='file://'+resolve(process.argv[1]))main().catch(()=>{process.exitCode=1;});
