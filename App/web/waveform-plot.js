import {esc} from './data-browser.js';

const colors=['#226da9','#d88018','#159677','#9363bd'];
const chart={width:960,height:340,left:68,top:18,right:936,bottom:292};
let clipSequence=0;
export function channelColor(channel,index=0){const match=/^CH([1-4])$/.exec(channel);return colors[match?Number(match[1])-1:index%4];}
const validDomain=d=>d&&Number.isFinite(d.start)&&Number.isFinite(d.end)&&d.end>d.start;
const validCalibration=c=>c&&Number.isFinite(c.x_increment_s)&&c.x_increment_s!==0&&Number.isFinite(c.x_origin_s)&&Number.isFinite(c.x_reference);
const atTime=(index,c)=>validCalibration(c)?(index-c.x_reference)*c.x_increment_s+c.x_origin_s:index;

/** Domain endpoints are physical sample times, not an exclusive native index. */
export function timeDomainForViews(views=[]){
 let start=Infinity,end=-Infinity,increment=1;
 for(const view of views){
  const c=view.calibration;
  if(validCalibration(c))increment=Math.min(increment,Math.abs(c.x_increment_s));
  const points=view.points;
  if(Array.isArray(points))for(const p of points){const t=atTime(p.x,c);if(!Number.isFinite(t))throw new Error('Invalid waveform calibration');start=Math.min(start,t);end=Math.max(end,t);}
  else if(Number.isSafeInteger(points)&&points>0){const a=atTime(0,c),b=atTime(points-1,c);start=Math.min(start,a,b);end=Math.max(end,a,b);}
 }
 if(!Number.isFinite(start))return null;
 return end>start?{start,end}:{start:start-increment/2,end:end+increment/2};
}

/** Map physical seconds to the bounded native [start,end) for each channel. */
export function sampleRangeForTime(domain,calibration,totalPoints){
 if(!validDomain(domain))throw new Error('Select a nonempty physical time range');
 if(!validCalibration(calibration))throw new Error('Valid time calibration is required');
 if(!Number.isSafeInteger(totalPoints)||totalPoints<1)throw new Error('Invalid source sample count');
 const index=t=>(t-calibration.x_origin_s)/calibration.x_increment_s+calibration.x_reference;
 const a=index(domain.start),b=index(domain.end),low=Math.min(a,b),high=Math.max(a,b);
 const tolerance=1e-8;
 if(high < -tolerance||low > totalPoints-1+tolerance)return null;
 const start=Math.max(0,Math.floor(low+tolerance)),end=Math.min(totalPoints,Math.ceil(high-tolerance)+1);
 return end>start?{start,end}:null;
}

export function roiFromDrag(from,to,box,range){if(!box||box.width<=0||Math.abs(to-from)<4||!range||range.end<=range.start)return null;const at=x=>Math.max(0,Math.min(1,(x-box.left)/box.width));return {start:Math.floor(range.start+Math.min(at(from),at(to))*(range.end-range.start)),end:Math.ceil(range.start+Math.max(at(from),at(to))*(range.end-range.start))};}

export function waveformBounds(views){let start=Infinity,end=-Infinity,min=Infinity,max=-Infinity;for(const view of views)for(const p of view.points||[]){if(!Number.isFinite(p.x)||!Number.isFinite(p.y))throw new Error('Invalid waveform viewport');start=Math.min(start,p.x);end=Math.max(end,p.x);min=Math.min(min,p.y);max=Math.max(max,p.y);}return Number.isFinite(start)?{start,end:end+1,min,max}:null;}

export function inspectPoint(view,index){const point=view?.points?.reduce((nearest,p)=>!nearest||Math.abs(p.x-index)<Math.abs(nearest.x-index)?p:nearest,null);if(!point)return null;const c=view.calibration;return {index:point.x,voltage_v:point.y,time_s:validCalibration(c)?atTime(point.x,c):null,raw_code:c&&c.y_increment_v!==0?(point.y-c.y_origin)/c.y_increment_v+c.y_reference:null};}

function unitFor(kind,magnitude){if(kind==='time')return magnitude>=1?{unit:'s',scale:1}:magnitude>=1e-3?{unit:'ms',scale:1e-3}:magnitude>=1e-6?{unit:'µs',scale:1e-6}:{unit:'ns',scale:1e-9};return magnitude>0&&magnitude<1?{unit:'mV',scale:1e-3}:{unit:'V',scale:1};}
function niceStep(span,target){const raw=span/target,power=10**Math.floor(Math.log10(raw)),fraction=raw/power;return (fraction<=1+1e-12?1:fraction<=2+1e-12?2:fraction<=5+1e-12?5:10)*power;}

/** Tick normalization affects labels alone; samples and calibration are immutable. */
export function waveformAxisTicks(start,end,{kind='voltage',target=8}={}){
 if(!Number.isFinite(start)||!Number.isFinite(end)||end<=start)throw new Error('Invalid waveform axis range');
 const step=niceStep(end-start,Math.max(2,Math.min(20,target))),{unit,scale}=unitFor(kind,Math.max(Math.abs(start),Math.abs(end),end-start));
 const decimals=Math.max(0,Math.min(12,-Math.floor(Math.log10(step/scale)+1e-10)));
 const ticks=[],first=Math.ceil(start/step-1e-9),last=Math.floor(end/step+1e-9);
 for(let i=first;i<=last&&ticks.length<64;i++){
  const value=i*step,display=Math.abs(value)<step*1e-9?0:value/scale;
  ticks.push({value,label:display.toFixed(decimals)});
 }
 return {start,end,step,unit,scale,ticks};
}

/** Returns the actual inner SVG chart in client coordinates, including letterboxing. */
export function waveformPlotGeometry(element){
 const box=element?.getBoundingClientRect?.();if(!box||box.width<=0||box.height<=0)return null;
 const data=element.dataset||{},width=Number(data.plotWidth)||chart.width,height=Number(data.plotHeight)||chart.height;
 const left=Number(data.plotLeft)||chart.left,top=Number(data.plotTop)||chart.top,right=Number(data.plotRight)||chart.right,bottom=Number(data.plotBottom)||chart.bottom;
 const scale=Math.min(box.width/width,box.height/height),offsetX=(box.width-width*scale)/2,offsetY=(box.height-height*scale)/2;
 const timeDomain={start:Number(data.timeStart),end:Number(data.timeEnd)};
 return {left:box.left+offsetX+left*scale,top:box.top+offsetY+top*scale,width:(right-left)*scale,height:(bottom-top)*scale,svg:{left,top,right,bottom},timeDomain:validDomain(timeDomain)?timeDomain:null};
}

export function renderWaveformPlot(views=[],{roi=null,timeRoi=null,analysis=null,cursor=null,timeDomain=null,viewport=null,referenceCalibration=null,clipId=null}={}){
 for(const view of views)if(!Array.isArray(view.points)||(view.points.length>8192))throw new Error('Waveform viewport must be bounded to 8192 points per channel.');
 if((analysis?.curve?.length||0)>8192||(analysis?.markers?.length||0)>8192)throw new Error('Analysis overlay must be bounded to 8192 points.');
 const bounds=waveformBounds(views);if(!bounds)return '<div class="waveform-empty">No waveform selected.</div>';
 const calibrated=views.every(v=>validCalibration(v.calibration));
 const reference=referenceCalibration||views.find(v=>v.channel===analysis?.channel)?.calibration||[...views].sort((a,b)=>a.channel.localeCompare(b.channel))[0]?.calibration;
 let domain=timeDomain;
 if(!validDomain(domain)&&validDomain(viewport))domain={start:atTime(viewport.start,reference),end:atTime(viewport.end-1,reference)};
 if(!validDomain(domain))domain=timeDomainForViews(views);
 if(!validDomain(domain))throw new Error('Invalid waveform time domain');
 const xAxis=waveformAxisTicks(domain.start,domain.end,{kind:calibrated?'time':'sample',target:10});
 if(!calibrated){xAxis.unit='';xAxis.scale=1;for(const t of xAxis.ticks)t.label=String(Number(t.value.toPrecision(7)));}
 const rawSpan=bounds.max-bounds.min,pad=rawSpan?rawSpan*.08:Math.max(Math.abs(bounds.max)*.1,.001);
 const yStep=niceStep(rawSpan+2*pad,8),lo=Math.floor((bounds.min-pad)/yStep)*yStep,hi=Math.ceil((bounds.max+pad)/yStep)*yStep;
 const yAxis=waveformAxisTicks(lo,hi,{kind:'voltage',target:Math.round((hi-lo)/yStep)});
 const x=t=>chart.left+(t-domain.start)/(domain.end-domain.start)*(chart.right-chart.left),y=v=>chart.bottom-(v-lo)/(hi-lo)*(chart.bottom-chart.top);
 const path=(points,c)=>points.map((p,i)=>{if(!Number.isFinite(p.x)||!Number.isFinite(p.y))throw new Error('Invalid waveform viewport');return `${i?'L':'M'}${x(atTime(p.x,c)).toFixed(2)},${y(p.y).toFixed(2)}`;}).join(' ');
 const selected=timeRoi||(roi?{start:atTime(roi.start,reference),end:atTime(roi.end-1,reference)}:null);
 const clip=esc(clipId||`waveform-clip-${++clipSequence}`),info=Number.isFinite(cursor)?inspectPoint(views.find(v=>v.channel===analysis?.channel)||views[0],cursor):null;
 const oldRange=validCalibration(reference)?{start:(domain.start-reference.x_origin_s)/reference.x_increment_s+reference.x_reference,end:(domain.end-reference.x_origin_s)/reference.x_increment_s+reference.x_reference+1}:domain;
 const gridX=xAxis.ticks.map(t=>`<line class="plot-grid plot-grid-x${t.label==='0'?' plot-zero':''}" x1="${x(t.value)}" x2="${x(t.value)}" y1="${chart.top}" y2="${chart.bottom}"/><text class="plot-tick" x="${x(t.value)}" y="${chart.bottom+19}" text-anchor="middle">${esc(t.label)}</text>`).join('');
 const gridY=yAxis.ticks.map(t=>`<line class="plot-grid plot-grid-y${t.label==='0'?' plot-zero':''}" x1="${chart.left}" x2="${chart.right}" y1="${y(t.value)}" y2="${y(t.value)}"/><text class="plot-tick" x="${chart.left-9}" y="${y(t.value)+4}" text-anchor="end">${esc(t.label)}</text>`).join('');
 return `<div class="waveform-panel"><div class="waveform-legend">${views.map((v,i)=>`<span style="--trace-color:${channelColor(v.channel,i)}">${esc(v.channel)}</span>`).join('')}<small>Drag to zoom · Right click to go back</small></div><svg class="waveform-plot" data-waveform-plot="true" data-range-start="${oldRange.start}" data-range-end="${oldRange.end}" data-time-start="${domain.start}" data-time-end="${domain.end}" data-plot-width="${chart.width}" data-plot-height="${chart.height}" data-plot-left="${chart.left}" data-plot-top="${chart.top}" data-plot-right="${chart.right}" data-plot-bottom="${chart.bottom}" viewBox="0 0 ${chart.width} ${chart.height}" role="img" aria-label="Waveform voltage versus ${calibrated?'calibrated time':'sample index'}"><defs><clipPath id="${clip}"><rect x="${chart.left}" y="${chart.top}" width="${chart.right-chart.left}" height="${chart.bottom-chart.top}"/></clipPath></defs>${gridX}${gridY}<rect class="plot-border" x="${chart.left}" y="${chart.top}" width="${chart.right-chart.left}" height="${chart.bottom-chart.top}"/><g data-waveform-traces="true" clip-path="url(#${clip})">${selected?`<rect class="plot-roi" x="${x(Math.min(selected.start,selected.end))}" y="${chart.top}" width="${Math.abs(x(selected.end)-x(selected.start))}" height="${chart.bottom-chart.top}"/>`:''}${views.map((v,i)=>`<path d="${path(v.points,v.calibration)}" fill="none" stroke="${channelColor(v.channel,i)}" stroke-width="1.4" data-channel="${esc(v.channel)}"/>`).join('')}${analysis?.curve?.length?`<path class="fit-curve" d="${path(analysis.curve,reference)}"/>`:''}${(analysis?.markers||[]).map(p=>`<circle cx="${x(atTime(p.x,reference))}" cy="${y(p.y)}" r="4" class="fit-marker"/>`).join('')}</g><g data-waveform-overlay="true" clip-path="url(#${clip})"></g><text class="plot-axis-label" x="${(chart.left+chart.right)/2}" y="334" text-anchor="middle">${calibrated?`Time (${xAxis.unit})`:'Absolute sample index'}</text><text class="plot-axis-label" transform="translate(14 155) rotate(-90)" text-anchor="middle">Voltage (${yAxis.unit})</text></svg>${info?`<div class="waveform-inspection">Sample ${info.index} · ${info.time_s===null?'Time unknown':Number((info.time_s/xAxis.scale).toPrecision(6))+' '+xAxis.unit} · ${Number((info.voltage_v/yAxis.scale).toPrecision(6))} ${yAxis.unit} · raw ${info.raw_code===null?'unknown':Number(info.raw_code.toPrecision(7))}</div>`:''}</div>`;
}
