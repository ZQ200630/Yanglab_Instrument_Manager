// Coordinates use observation timestamps; repainting only moves the time window.
export function gainTrendModel(history=[],{nowMs=performance.now(),windowS=300}={}){
 const span=([60,300,900].includes(Number(windowS))?Number(windowS):300)*1000;
 const end=Number.isFinite(nowMs)?nowMs:0,start=end-span;
 const visible=history.filter(p=>Number.isFinite(p?.observedAtMs)&&p.observedAtMs>=start&&p.observedAtMs<=end);
 const measured=visible.filter(p=>!p.gap&&Number.isFinite(p.temperature_c));
 const ys=measured.flatMap(p=>Number.isFinite(p.target_c)?[p.temperature_c,p.target_c]:[p.temperature_c]);
 const low=ys.length?Math.min(...ys):15,high=ys.length?Math.max(...ys):40;
 const padding=Math.max(.2,(high-low)*.14),yMin=low-padding,yMax=high+padding;
 const x=t=>52+(t-start)/span*560,y=v=>188-(v-yMin)/(yMax-yMin)*168;
 const segments=[],targets=[];let segment=null,target=null,connection=null,last=-Infinity;
 for(const p of visible){
  if(p.observedAtMs<=last){segment=null;target=null;continue;}last=p.observedAtMs;
  if(p.gap||!Number.isFinite(p.temperature_c)){segment=null;target=null;continue;}
  if(connection!==null&&p.connection_id!==connection){segment=null;target=null;}connection=p.connection_id;
  if(!segment){segment=[];segments.push(segment);}segment.push({...p,x:x(p.observedAtMs),y:y(p.temperature_c)});
  if(Number.isFinite(p.target_c)){if(!target){target=[];targets.push(target);}target.push({x:x(p.observedAtMs),y:y(p.target_c)});}else target=null;
 }
 return {segments,targets,yMin,yMax,windowS:span/1000,count:segments.flat().length};
}
export function renderGainTrend(history,options){
 const model=gainTrendModel(history,options),point=p=>`${p.x.toFixed(2)},${p.y.toFixed(2)}`;
 const grid=Array.from({length:5},(_,i)=>{const y=20+i*42,value=model.yMax-i*(model.yMax-model.yMin)/4;return `<line x1="52" x2="612" y1="${y}" y2="${y}" class="gain-trend-grid"/><text x="42" y="${y+4}" text-anchor="end">${value.toFixed(1)}</text>`;}).join('');
 return `<svg class="gain-trend" data-trend="gain-temperature" data-observations="${model.count}" viewBox="0 0 640 230" role="img" aria-label="Temperature history, last ${model.windowS/60} minutes. ${model.count} observed samples."><text x="14" y="12">°C</text>${grid}${model.targets.map(s=>`<polyline class="gain-trend-target" points="${s.map(point).join(' ')}"/>`).join('')}${model.segments.map(s=>`<polyline class="gain-trend-measured" points="${s.map(point).join(' ')}"/>${s.length===1?`<circle class="gain-trend-dot" cx="${s[0].x}" cy="${s[0].y}" r="3"/>`:''}`).join('')}<text x="52" y="214">−${model.windowS/60} min</text><text x="332" y="214" text-anchor="middle">−${model.windowS/120} min</text><text x="612" y="214" text-anchor="end">Now</text>${model.count?'':'<text class="gain-trend-empty" x="332" y="108" text-anchor="middle">Waiting for temperature samples</text>'}</svg>`;
}
