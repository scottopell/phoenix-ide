import test from 'node:test';import assert from 'node:assert/strict';import{runInNewContext}from'node:vm';import{geometry}from'./history-geometry-repro.mjs';
test('one physical observation preserves all four signed deltas without leaking identity or text',()=>{
 const box=(top,left,height,width)=>({top,left,height,width,bottom:top+height,right:left+width});
 const node=b=>({getBoundingClientRect:()=>b,getClientRects:()=>[b],isConnected:true,parentElement:null});
 const scroller={...node(box(100,10,200,300)),clientHeight:200,clientWidth:300};
 const row={...node(box(99,9,20,302)),querySelector:()=>null};
 const child={...node(box(99,9,20,302)),id:'message-PRIVATE-ID',getAttribute:k=>k==='data-message-id'?'PRIVATE-ID':null,closest:()=>row,parentElement:row};
 const context={document:{querySelector:()=>scroller,querySelectorAll:()=>[child],documentElement:node(box(0,0,900,1360))},
 innerWidth:1360,innerHeight:900,devicePixelRatio:1,visualViewport:null,getComputedStyle:()=>({display:'block',visibility:'visible'}),
 location:{href:'http://127.0.0.1/?source_transcript=PRIVATE-SOURCE',pathname:'/',search:'?source_transcript=PRIVATE-SOURCE',hash:''},URL};
 const result=runInNewContext(`(${geometry.toString()})`,context,{timeout:1000})({id:'PRIVATE-ID',source:'PRIVATE-SOURCE',route:'/?source_transcript=PRIVATE-SOURCE'});
 assert.equal(result.child.edges.top,-1);assert.equal(result.child.edges.left,-1);assert.equal(result.child.edges.right,-1);assert.equal(result.child.edges.bottom,181);
 assert.equal(result.identity.sourcepin_matches,true);assert.equal(result.identity.original_route,true);
 assert.ok(!JSON.stringify(result).includes('PRIVATE'));
});
