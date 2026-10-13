import test from 'node:test';
import assert from 'node:assert/strict';
import * as plot from '../web/waveform-plot.js';

const cal={x_increment_s:1e-6,x_origin_s:0,x_reference:0,y_increment_v:.01,y_origin:0,y_reference:128};
const view=(channel,calibration=cal)=>({channel,calibration,total_points:101,points:[{x:0,y:-.002},{x:100,y:.002}]});
const paths=html=>[...html.matchAll(/<path d="([^"]+)" fill="none" stroke="[^"]+"[^>]*data-channel="([^"]+)"/g)].map(m=>({path:m[1],channel:m[2]}));

test('scope axes have regular time/voltage ticks and adaptive SI units without altering samples',()=>{
 const samples=view('CH1');const snapshot=structuredClone(samples);
 const html=plot.renderWaveformPlot([samples],{timeDomain:{start:0,end:.0001}});
 assert.match(html,/Time \(µs\)/);assert.match(html,/Voltage \(mV\)/);
 assert.ok((html.match(/class="plot-grid plot-grid-x"/g)||[]).length>=5);
 assert.ok((html.match(/class="plot-grid plot-grid-y"/g)||[]).length>=5);
 assert.deepEqual(samples,snapshot);
 assert.match(plot.renderWaveformPlot([view('CH1',{...cal,x_increment_s:1e-9})]),/Time \(ns\)/);
 assert.match(plot.renderWaveformPlot([view('CH1',{...cal,x_increment_s:1e-3})]),/Time \(ms\)/);
 assert.match(plot.renderWaveformPlot([view('CH1',{...cal,x_increment_s:1})]),/Time \(s\)/);
});

test('near-zero display ticks normalize floating point residuals and use nice intervals',()=>{
 assert.equal(typeof plot.waveformAxisTicks,'function');
 const axis=plot.waveformAxisTicks(-3e-6+1e-22,3e-6+1e-22,{kind:'time',target:6});
 assert.equal(axis.unit,'µs');assert.equal(axis.step,1e-6);
 assert.deepEqual(axis.ticks.map(t=>t.label),['-3','-2','-1','0','1','2','3']);
 assert.ok(!axis.ticks.some(t=>t.label==='-0'));
});

test('physical time domain and calibrated path endpoints stay fixed when CH1 is hidden',()=>{
 const one=view('CH1'),two=view('CH2',{...cal,x_increment_s:2e-6,x_origin_s:-20e-6});
 const options={timeDomain:{start:-20e-6,end:180e-6},referenceCalibration:cal};
 const together=paths(plot.renderWaveformPlot([one,two],options));
 const hidden=paths(plot.renderWaveformPlot([two],options));
 assert.equal(together.length,2);assert.equal(hidden.length,1);
 assert.equal(together[1].path,hidden[0].path);
 assert.match(hidden[0].path,/^M68\.00,/);assert.match(hidden[0].path,/L936\.00,/);
});

test('physical selection maps to each channels own native indices and clamps to its source',()=>{
 assert.equal(typeof plot.sampleRangeForTime,'function');
 assert.deepEqual(plot.sampleRangeForTime({start:20e-6,end:50e-6},cal,101),{start:20,end:51});
 assert.deepEqual(plot.sampleRangeForTime({start:20e-6,end:50e-6},{...cal,x_increment_s:2e-6,x_origin_s:-20e-6},101),{start:20,end:36});
 assert.deepEqual(plot.sampleRangeForTime({start:-1,end:1},cal,101),{start:0,end:101});
 assert.equal(plot.sampleRangeForTime({start:1,end:2},cal,101),null);
 assert.throws(()=>plot.sampleRangeForTime({start:0,end:1},{...cal,x_increment_s:0},101),/calibration/i);
});

test('time extent uses calibrated endpoints, independent of channel ordering',()=>{
 assert.equal(typeof plot.timeDomainForViews,'function');
 const two=view('CH2',{...cal,x_increment_s:2e-6,x_origin_s:-20e-6});
 const extent=plot.timeDomainForViews([two,view('CH1')]);
 assert.equal(extent.start,-20e-6);assert.ok(Math.abs(extent.end-180e-6)<1e-18);
});

test('metadata extent retains all channels and boundary rounding never drops the last sample',()=>{
 const metadata=[{channel:'CH1',points:101,calibration:cal},{channel:'CH2',points:101,calibration:{...cal,x_origin_s:-20e-6,x_increment_s:2e-6}}];
 const domain=plot.timeDomainForViews(metadata);
 assert.equal(domain.start,-20e-6);assert.ok(Math.abs(domain.end-180e-6)<1e-18);
 assert.deepEqual(plot.sampleRangeForTime({start:180e-6,end:181e-6},metadata[1].calibration,101),{start:100,end:101});
});

test('negative time increments map a physical ROI to ordered exclusive native bounds',()=>{
 assert.deepEqual(plot.sampleRangeForTime({start:20e-6,end:50e-6},{...cal,x_increment_s:-1e-6,x_origin_s:100e-6},101),{start:50,end:81});
});

test('multiple plots use separate clip IDs and reject excessive fit overlays',()=>{
 const one=plot.renderWaveformPlot([view('CH1')]),two=plot.renderWaveformPlot([view('CH1')]);
 assert.notEqual(one.match(/clipPath id="([^"]+)/)?.[1],two.match(/clipPath id="([^"]+)/)?.[1]);
 assert.throws(()=>plot.renderWaveformPlot([view('CH1')],{analysis:{curve:Array.from({length:8193},()=>({x:0,y:0}))}}),/bounded/i);
});
