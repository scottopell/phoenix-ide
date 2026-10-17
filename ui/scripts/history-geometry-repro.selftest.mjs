import test from 'node:test';import assert from 'node:assert/strict';import{runInNewContext}from'node:vm';import{geometry,installWriteCapture}from'./history-geometry-repro.mjs';
test('one physical observation preserves all four signed deltas without leaking identity or text',()=>{
 const box=(top,left,height,width)=>({top,left,height,width,bottom:top+height,right:left+width});
 const node=b=>({getBoundingClientRect:()=>b,getClientRects:()=>[b],isConnected:true,parentElement:null});
 const scroller={...node(box(100,10,200,300)),clientHeight:200,clientWidth:300};
 const row={...node(box(99,9,20,302)),querySelector:()=>null};
 const child={...node(box(99,9,20,302)),id:'message-PRIVATE-ID',getAttribute:k=>k==='data-message-id'?'PRIVATE-ID':null,closest:()=>row,parentElement:row};
 const context={document:{querySelector:()=>scroller,querySelectorAll:selector=>selector==='textarea#message-input'?[]:[child],documentElement:node(box(0,0,900,1360))},
 innerWidth:1360,innerHeight:900,devicePixelRatio:1,visualViewport:null,getComputedStyle:()=>({display:'block',visibility:'visible'}),
 location:{href:'http://127.0.0.1/?source_transcript=PRIVATE-SOURCE',pathname:'/',search:'?source_transcript=PRIVATE-SOURCE',hash:''},URL};
 const result=runInNewContext(`(${geometry.toString()})`,context,{timeout:1000})({id:'PRIVATE-ID',source:'PRIVATE-SOURCE',route:'/?source_transcript=PRIVATE-SOURCE'});
 assert.equal(result.child.edges.top,-1);assert.equal(result.child.edges.left,-1);assert.equal(result.child.edges.right,-1);assert.equal(result.child.edges.bottom,181);
 assert.equal(result.identity.sourcepin_matches,true);assert.equal(result.identity.original_route,true);
 assert.ok(!JSON.stringify(result).includes('PRIVATE'));
});

test('write observer preserves requested values and native return while recording immediate readback and bounded private-safe events',()=>{
 class Element{constructor(id){this.id=id;this.top=0;}get scrollTop(){return this.top;}set scrollTop(value){this.top=Math.round(value);}closest(){return null;}}
 class ResizeObserver{constructor(cb){this.cb=cb;}}
 const window={ResizeObserver};const context={window,Element,ResizeObserver};
 const install=runInNewContext(`(${installWriteCapture.toString()})`,context,{timeout:1000});
 install({input:{id:'PRIVATE'},geometrySource:'input=>({top:0})'});
 const scroller=new Element('messages');scroller.scrollTop=42.5;assert.equal(scroller.scrollTop,43);
 const other=new Element('PRIVATE');other.scrollTop=1.5;assert.equal(other.scrollTop,2);
 for(let i=0;i<140;i++)scroller.scrollTop=i+.5;
 const capture=window.__historyGeometryRead();assert.equal(capture.events.length,128);assert.equal(capture.dropped,13);
 assert.equal(capture.events[0].requested,42.5);assert.equal(capture.events[0].applied,43);
 assert.ok(!JSON.stringify(capture).includes('PRIVATE'));
 assert.equal(capture.events[0].callsite.owner,'compiled_unresolved');
});
test('instrumentation geometry failure cannot alter native assignment or ResizeObserver callback delivery',()=>{
 class Element{constructor(){this.id='messages';}get scrollTop(){return this.top??0;}set scrollTop(value){this.top=value;}}
 class NativeObserver{constructor(cb){this.deliver=cb;}}
 const window={ResizeObserver:NativeObserver};const context={window,Element};
 runInNewContext(`(${installWriteCapture.toString()})`,context,{timeout:1000})({input:{},geometrySource:'()=>{throw new Error("PRIVATE");}'});
 const scroller=new Element();scroller.scrollTop=42.5;assert.equal(scroller.scrollTop,42.5);
 let called=0;const ro=new window.ResizeObserver(()=>{called++;});ro.deliver([{target:scroller,contentRect:{height:1,width:1}}],ro);
 assert.equal(called,1);assert.equal(window.__historyGeometryRead().events[0].geometry.instrumentation_geometry_failed,true);
 assert.ok(!JSON.stringify(window.__historyGeometryRead()).includes('PRIVATE'));
});

test('observer preserves native callback receiver, identity, exceptions and method returns',()=>{
 const sentinel={native:true},thrown=new Error('UNIT native exception');
 class Element{constructor(){this.id='messages';}get scrollTop(){return 0;}set scrollTop(value){}scrollTo(){return sentinel;}scrollBy(){throw thrown;}}
 class NativeObserver{constructor(cb){this.deliver=cb;}}
 const window={ResizeObserver:NativeObserver};
 runInNewContext(`(${installWriteCapture.toString()})`,{window,Element},{timeout:1000})({input:{},geometrySource:'()=>({})'});
 const el=new Element();assert.equal(el.scrollTo(),sentinel);assert.throws(()=>el.scrollBy(),e=>e===thrown);
 const entries=[];let invoked=0;
 const ro=new window.ResizeObserver(function(observed,observer){assert.equal(this,ro);assert.equal(observed,entries);assert.equal(observer,ro);invoked++;throw thrown;});
 assert.throws(()=>ro.deliver(entries,ro),e=>e===thrown);assert.equal(invoked,1);
 assert.equal(window.__historyGeometryRead().installed,true);
});
