import {createRequire} from 'node:module';
import {resolve} from 'node:path';
import {readFileSync,writeFileSync} from 'node:fs';
import {authenticateOwnedContext} from './history-joint-telemetry.mjs';
import {observePhysicalHistoricalRow} from './history-postfault-baseline.mjs';
export function geometry({id, source, route}) {
  const rect=e=>{if(!e)return null;const b=e.getBoundingClientRect();return Object.fromEntries(['top','bottom','left','right','width','height'].map(k=>[k,Number.isFinite(b[k])?b[k]:null]));};
  const scroller=document.querySelector('#messages'),sr=rect(scroller);
  const matches=[...document.querySelectorAll('[id], [data-message-id]')].filter(e=>e.id==='message-'+id||e.getAttribute('data-message-id')===id);
  const child=matches.length===1?matches[0]:null,row=child?.closest('[data-virtual-key]');
  const clip=sr?{top:Math.max(0,sr.top),bottom:Math.min(innerHeight,sr.bottom),left:Math.max(0,sr.left),right:Math.min(innerWidth,sr.right)}:null;
  const inspect=e=>{if(!e)return null;const b=rect(e),s=getComputedStyle(e);return {rect:b,edges:clip?{top:b.top-clip.top,bottom:clip.bottom-b.bottom,left:b.left-clip.left,right:clip.right-b.right}:null,
    clientRects:e.getClientRects().length,connected:e.isConnected,styles:Object.fromEntries(['display','visibility','overflowX','overflowY','contain','position','transform','boxSizing','borderTopWidth','borderBottomWidth','borderLeftWidth','borderRightWidth','paddingTop','paddingBottom','paddingLeft','paddingRight'].map(k=>[k,s[k]]))};};
  const ancestors=[];for(let e=child?.parentElement;e&&ancestors.length<8;e=e.parentElement)ancestors.push(inspect(e));
  return {viewport:{innerWidth,innerHeight,devicePixelRatio,visualViewport:visualViewport?{width:visualViewport.width,height:visualViewport.height,offsetTop:visualViewport.offsetTop,offsetLeft:visualViewport.offsetLeft,scale:visualViewport.scale}:null},
    document:rect(document.documentElement),scroller:inspect(scroller),scroll:scroller?Object.fromEntries(['clientTop','clientLeft','clientHeight','clientWidth','scrollTop','scrollLeft','scrollHeight','scrollWidth'].map(k=>[k,scroller[k]])):null,
    clip,child:inspect(child),row:inspect(row),inner:inspect(row?.querySelector('.virtual-transcript-row')),ancestors,
    identity:{physical_matches:matches.length,fixture_id_matches:!!child,alias_agrees:!child?.getAttribute('data-message-id')||child.getAttribute('data-message-id')===id,
      sourcepin_matches:new URL(location.href).searchParams.get('source_transcript')===source,original_route:location.pathname+location.search+location.hash===route}};
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
    page=await context.newPage();let reports=0;page.on('response',r=>{if(new URL(r.url()).pathname==='/api/telemetry/conversation-view'&&r.status()===204)reports++;});
    await page.goto(process.env.HISTORY_BASE_URL+input.route,{waitUntil:'domcontentloaded',timeout:20000});
    const deadline=Date.now()+20000;
    while(reports===0&&Date.now()<deadline)await page.evaluate(()=>new Promise(r=>requestAnimationFrame(r)));
    receipt.report204_count=reports;
    for(let frame=0;frame<5;frame++){
      if(frame)await page.evaluate(()=>new Promise(r=>requestAnimationFrame(r)));
      const observation=await page.evaluate(async ({geometrySource,oracleSource,input})=>{
        const data=(0,eval)('('+geometrySource+')')(input);
        const physical=(0,eval)('('+oracleSource+')')({known:[{persistedMessageID:input.id,sourceRow:input.source}]});
        return {...data,strict_containment:physical!==null};
      },{geometrySource:geometry.toString(),oracleSource:observePhysicalHistoricalRow.toString(),input});
      receipt.observations.push(observation);save();
    }
    await page.locator('#messages').screenshot({path:resolve(out,'messages.png'),timeout:5000});receipt.status=reports>0&&receipt.observations.length===5&&receipt.observations.every(o=>o.identity.fixture_id_matches)?'captured':'capture_prerequisites_missing';save();
  }catch{receipt.failure='bounded_geometry_operation_failed';save();}
  finally{try{await context?.close();await browser?.close();}catch{receipt.cleanup_failure=true;}clearTimeout(watchdog);save();}
  if(receipt.status!=='captured')process.exitCode=1;
}
if(process.argv[1]&&import.meta.url==='file://'+resolve(process.argv[1]))main().catch(()=>{process.exitCode=1;});
