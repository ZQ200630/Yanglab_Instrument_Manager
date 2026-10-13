import {waveformPlotGeometry} from './waveform-plot.js';

const validRange=r=>r&&Number.isFinite(r.start)&&Number.isFinite(r.end)&&r.end>r.start;
const clamp=value=>Math.max(0,Math.min(1,value));

/**
 * Attach once to a rendered SVG. Page owners retain zoom history and guard
 * asynchronous replies; callbacks receive physical seconds, never sample IDs.
 * update() cancels an in-flight gesture if its source key or viewport changes.
 */
export function createWaveformInteraction({element,...initial}={}){
 if(!element?.addEventListener)throw new Error('A waveform SVG element is required');
 let options={color:'#226da9',minimumDragPx:4,enabled:true,...initial},drag=null,destroyed=false;
 function cancel(){
  const previous=drag;drag=null;if(!previous)return;
  previous.overlay.remove();
  try{if(element.hasPointerCapture?.(previous.pointerId))element.releasePointerCapture(previous.pointerId);}catch{}
 }
 function fraction(clientX,geometry){return clamp((clientX-geometry.left)/geometry.width);}
 function paint(clientX){
  if(!drag)return;const {geometry,from,overlay}=drag,to=fraction(clientX,geometry),left=Math.min(from,to),width=Math.abs(to-from);
  overlay.setAttribute('x',geometry.svg.left+left*(geometry.svg.right-geometry.svg.left));
  overlay.setAttribute('width',width*(geometry.svg.right-geometry.svg.left));
 }
 function down(event){
  if(destroyed||!options.enabled||drag||event.button!==0||event.isPrimary===false||!Number.isInteger(event.pointerId))return;
  const geometry=waveformPlotGeometry(element),viewport=options.viewport||geometry?.timeDomain;
  if(!geometry||!validRange(viewport)||event.clientX<geometry.left||event.clientX>geometry.left+geometry.width||event.clientY<geometry.top||event.clientY>geometry.top+geometry.height)return;
  const host=element.querySelector('[data-waveform-overlay]');if(!host)return;
  const overlay=element.ownerDocument.createElementNS('http://www.w3.org/2000/svg','rect');
  overlay.setAttribute('class','plot-drag-overlay');overlay.setAttribute('aria-hidden','true');overlay.setAttribute('pointer-events','none');
  overlay.setAttribute('fill',options.color);overlay.setAttribute('fill-opacity','.18');overlay.setAttribute('stroke',options.color);overlay.setAttribute('stroke-width','1.5');
  overlay.setAttribute('y',geometry.svg.top);overlay.setAttribute('height',geometry.svg.bottom-geometry.svg.top);
  host.appendChild(overlay);drag={pointerId:event.pointerId,from:fraction(event.clientX,geometry),geometry,viewport:{...viewport},overlay};
  try{element.setPointerCapture(event.pointerId);}catch{cancel();return;}
  paint(event.clientX);event.preventDefault();
 }
 function move(event){if(!drag||event.pointerId!==drag.pointerId)return;if(event.buttons===0){cancel();return;}paint(event.clientX);event.preventDefault();}
 function up(event){
  if(!drag||event.pointerId!==drag.pointerId||event.button!==0)return;
  const previous=drag,to=fraction(event.clientX,previous.geometry),width=Math.abs(to-previous.from)*previous.geometry.width;
  cancel();event.preventDefault();
  if(width<options.minimumDragPx)return;
  const {viewport,from}=previous,span=viewport.end-viewport.start;
  const range={start:viewport.start+Math.min(from,to)*span,end:viewport.start+Math.max(from,to)*span};
  if(validRange(range))options.onZoom?.(range);
 }
 function aborted(event){if(drag&&event.pointerId===drag.pointerId)cancel();}
 function back(event){event.preventDefault();cancel();if(!destroyed&&options.enabled)options.onBack?.();}
 const handlers={pointerdown:down,pointermove:move,pointerup:up,pointercancel:aborted,lostpointercapture:aborted,contextmenu:back};
 for(const [name,handler]of Object.entries(handlers))element.addEventListener(name,handler);
 return {
  cancel,
  update(next={}){
   if(destroyed)return;
   const newViewport=next.viewport;
   if(('key'in next&&next.key!==options.key)||('enabled'in next&&!next.enabled)||newViewport&&(newViewport.start!==options.viewport?.start||newViewport.end!==options.viewport?.end))cancel();
   options={...options,...next};
  },
  destroy(){if(destroyed)return;destroyed=true;for(const [name,handler]of Object.entries(handlers))element.removeEventListener(name,handler);cancel();},
 };
}
