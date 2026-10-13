import test from 'node:test';
import assert from 'node:assert/strict';
import {waveformPlotGeometry} from '../web/waveform-plot.js';
const interaction=await import('../web/waveform-interaction.js').catch(e=>{if(e.code==='ERR_MODULE_NOT_FOUND')return {};throw e;});

// A bounded DOM boundary for node: the real controller owns the handlers, rect,
// capture lifecycle and callbacks; this supplies DOM primitives, never samples.
function element(){
 const handlers=new Map(),children=[],captures=new Set();
 const overlay={appendChild:node=>{children.push(node);node.parentNode=overlay;return node;},removeChild:node=>children.splice(children.indexOf(node),1)};
 const el={dataset:{timeStart:'0',timeEnd:'.0001'},style:{},isConnected:true,
  getBoundingClientRect:()=>({left:100,top:50,width:960,height:340}),
  querySelector:selector=>selector==='[data-waveform-overlay]'?overlay:null,
  ownerDocument:{createElementNS:()=>({attributes:{},style:{setProperty(k,v){this[k]=v;}},setAttribute(k,v){this.attributes[k]=String(v);},remove(){this.parentNode?.removeChild(this);}})},
  addEventListener(name,fn){handlers.set(name,fn);},removeEventListener(name,fn){if(handlers.get(name)===fn)handlers.delete(name);},
  setPointerCapture:id=>captures.add(id),hasPointerCapture:id=>captures.has(id),releasePointerCapture:id=>captures.delete(id),
  fire(name,values={}){const event={clientX:168,clientY:150,button:0,buttons:1,pointerId:7,isPrimary:true,preventDefault(){this.defaultPrevented=true;},...values};handlers.get(name)?.(event);return event;},
  children,captures,handlers};
 return el;
}
const create=options=>{assert.equal(typeof interaction.createWaveformInteraction,'function');return interaction.createWaveformInteraction(options);};

test('geometry excludes axes and SVG letterboxing from physical pointer coordinates',()=>{
 const el=element();el.getBoundingClientRect=()=>({left:100,top:50,width:960,height:500});
 const geometry=waveformPlotGeometry(el);
 assert.equal(geometry.left,168);assert.equal(geometry.top,148);assert.equal(geometry.width,868);assert.equal(geometry.height,274);
});

test('drag paints a colored live horizontal overlay, clamps reversal, then zooms on release',()=>{
 const el=element(),zooms=[];const control=create({element:el,viewport:{start:0,end:.0001},color:'#d88018',onZoom:range=>zooms.push(range)});
 el.fire('pointerdown',{clientX:819,clientY:150});assert.ok(el.captures.has(7));
 el.fire('pointermove',{clientX:385});assert.equal(zooms.length,0);assert.equal(el.children.length,1);
 const rect=el.children[0];assert.equal(rect.attributes.x,'285');assert.equal(rect.attributes.width,'434');assert.equal(rect.attributes.fill,'#d88018');
 el.fire('pointerup',{clientX:385,buttons:0});assert.equal(zooms.length,1);assert.ok(Math.abs(zooms[0].start-.000025)<1e-18);assert.ok(Math.abs(zooms[0].end-.000075)<1e-18);
 assert.equal(el.children.length,0);assert.equal(el.captures.size,0);control.destroy();assert.equal(el.handlers.size,0);
});

test('pointer cancellation, identity changes and lost capture discard drag without zoom',()=>{
 for(const event of ['pointercancel','lostpointercapture','update','destroy']){
  const el=element(),zooms=[];const control=create({element:el,viewport:{start:0,end:1},key:'frame1',onZoom:range=>zooms.push(range)});
  el.fire('pointerdown',{clientX:300});el.fire('pointermove',{clientX:600});
  if(event==='update')control.update({key:'frame2',viewport:{start:0,end:1}});else if(event==='destroy')control.destroy();else el.fire(event);
  el.fire('pointerup',{clientX:600,buttons:0});assert.equal(zooms.length,0,event);assert.equal(el.children.length,0,event);assert.equal(el.captures.size,0,event);control.destroy();
 }
});

test('only the matching primary pointer can finish a nonempty in-chart drag',()=>{
 const el=element(),zooms=[];const control=create({element:el,viewport:{start:0,end:1},onZoom:r=>zooms.push(r)});
 for(const values of [{button:2},{button:1},{isPrimary:false},{clientY:60},{clientX:120}])el.fire('pointerdown',values);
 el.fire('pointerup',{clientX:600,buttons:0});assert.equal(zooms.length,0);
 el.fire('pointerdown',{clientX:300});el.fire('pointerup',{clientX:600,pointerId:8,buttons:0});assert.equal(zooms.length,0);assert.equal(el.children.length,1);
 el.fire('pointerup',{clientX:302,buttons:0});assert.equal(zooms.length,0);assert.equal(el.children.length,0);
 control.destroy();
});

test('right click invokes one history step and suppresses browser context navigation',()=>{
 const el=element();let depth=2;const control=create({element:el,viewport:{start:0,end:1},onBack:()=>{depth=Math.max(0,depth-1);}});
 for(let i=0;i<3;i++){el.fire('pointerdown',{button:2});const event=el.fire('contextmenu',{button:2});assert.equal(event.defaultPrevented,true);assert.equal(depth,Math.max(0,1-i));}
 control.destroy();
});

test('drag outside chart clamps to source extent without issuing duplicate callbacks',()=>{
 const el=element(),zooms=[];const control=create({element:el,viewport:{start:10,end:20},onZoom:r=>zooms.push(r)});
 el.fire('pointerdown',{clientX:300});el.fire('pointermove',{clientX:-100});el.fire('pointerup',{clientX:2000,buttons:0});el.fire('pointerup',{clientX:2000,buttons:0});
 assert.equal(zooms.length,1);assert.equal(zooms[0].end,20);assert.ok(zooms[0].start>10&&zooms[0].start<12);control.destroy();
});
