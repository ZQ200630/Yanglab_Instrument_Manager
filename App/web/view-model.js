/** Display calculations shared by the panels. The Python drivers own safety. */
import { gainEvidence } from './control-state.js';
import {rawCursor} from './osa.js';

const axes = ['x', 'y', 'z'];

function finite(value) {
  return typeof value === 'number' && Number.isFinite(value);
}

export function sampleAgeLabel(value, source = 'Host status sample') {
  return finite(value) && value >= 0 ? `${source} ${value.toFixed(1)} s ago` : `${source} age unknown`;
}

export function osaCursorIndex(trace, fraction) {
  if(trace?.verified===true)return rawCursor(trace,fraction)?.index??null;
  const xs = trace?.wavelength_nm;
  const ys = trace?.power_dbm;
  if (!Array.isArray(xs) || !Array.isArray(ys) || xs.length !== ys.length || !xs.length) return null;
  let minimum = Infinity, maximum = -Infinity;
  for (let index = 0; index < xs.length; index += 1) {
    if (!finite(xs[index]) || !finite(ys[index])) continue;
    minimum = Math.min(minimum, xs[index]);
    maximum = Math.max(maximum, xs[index]);
  }
  if (!finite(minimum)) return null;
  const position = Math.max(0, Math.min(1, finite(fraction) ? fraction : 0));
  const wavelength = minimum + position * (maximum - minimum);
  let nearest = null, distance = Infinity;
  for (let index = 0; index < xs.length; index += 1) {
    if (!finite(xs[index]) || !finite(ys[index])) continue;
    const candidate = Math.abs(xs[index] - wavelength);
    if (candidate < distance) { distance = candidate; nearest = index; }
  }
  return nearest;
}

export function appendTelemetry(history, sample, role) {
  const previous = Array.isArray(history) ? history : [];
  if (role === 'gain') {
    const field = gainEvidence(sample, 'temperature_c');
    if (field.quality !== 'fresh' || !finite(field.value) || !field.connection_id ||
        !Number.isSafeInteger(field.revision)) return previous;
    const last = previous.at(-1);
    if (last?.connection_id === field.connection_id && last.revision >= field.revision) return previous;
    const retained = last && last.connection_id !== field.connection_id ? [] : previous;
    return [...retained, { connection_id: field.connection_id, revision: field.revision,
      temperature_c: field.value }].slice(-60);
  }
  if (!finite(sample?.received_at)) return previous;
  const last = previous.at(-1);
  if (last && sample.received_at <= last.received_at) return previous;
  let entry;
  if (role === 'voltage' && Array.isArray(sample.voltage_v)
      && sample.voltage_v.length === 8 && sample.voltage_v.every(finite)) {
    entry = { received_at: sample.received_at, voltage_v: [...sample.voltage_v] };
  } else {
    return previous;
  }
  return [...previous, entry].slice(-60);
}

export function describeStage(status) {
  const side = status?.side;
  const towardChip = side === 'left' ? '+X' : side === 'right' ? '−X' : 'Unknown';
  const voltage = axes.map((axis) => {
    const value = status?.observed_voltage_v?.[axis];
    return finite(value) ? `${value.toFixed(3)} V` : 'Unknown';
  });
  const estimate = status?.estimated_position_um;
  const serials = {left: '2110148249-10', right: '160721175410'};
  const known = Object.hasOwn(serials, side)
    && (status?.serial_number ?? status?.serial) === serials[side]
    && status?.available === true && status?.baseline_known === true
    && status?.nominal_authorized === true && status?.restricted !== true
    && !status?.fault && !status?.status_error
    && estimate && axes.every((axis) => finite(estimate[axis]));
  const position = known
    ? axes.map((axis) => `${axis.toUpperCase()} ${estimate[axis].toFixed(3)} µm`).join(' · ')
    : 'Unknown';
  return {
    side,
    serial: status?.serial || status?.serial_number || 'Unknown',
    available: status?.available === true,
    voltage,
    position,
    towardChip,
    canMove: Boolean(known),
    fault: status?.fault || null,
    restricted: status?.restricted === true,
  };
}

export function previewStageMove(side, displacement) {
  if (!['left', 'right'].includes(side)) return { allowed: false, reason: 'Select a stage side' };
  const vector = axes.map((axis) => displacement?.[axis]);
  if (!vector.every(finite)) return { allowed: false, reason: 'Enter finite X/Y/Z values' };
  if (vector.every((value) => value === 0)) return { allowed: false, reason: 'Enter a nonzero move' };
  if (vector.some((value) => Math.abs(value) > 1)) {
    return { allowed: false, reason: 'Each axis is limited to 1.0 µm per move' };
  }
  const toward = vector[0] * (side === 'left' ? 1 : -1);
  if (toward > 0.2) return { allowed: false, reason: 'Toward-chip X is limited to 0.2 µm per move' };
  const label = axes.map((axis, index) => `${axis.toUpperCase()} ${vector[index] >= 0 ? '+' : ''}${vector[index].toFixed(3)} µm`).join(' · ');
  return { allowed: true, reason: toward > 0 ? 'Toward chip' : 'Within per-axis limits', label };
}

export function rotateStageView(side, view, key) {
  if (side !== 'left' && side !== 'right') return null;
  const defaultYaw = side === 'left' ? -35 : 35;
  const yaw = finite(view?.yaw) ? view.yaw : defaultYaw;
  const pitch = finite(view?.pitch) ? view.pitch : -25;
  if (key === 'Home') return { yaw: defaultYaw, pitch: -25 };
  const delta = {
    ArrowLeft: { yaw: -5, pitch: 0 },
    ArrowRight: { yaw: 5, pitch: 0 },
    ArrowUp: { yaw: 0, pitch: 5 },
    ArrowDown: { yaw: 0, pitch: -5 },
  }[key];
  if (!delta) return null;
  return {
    yaw: Math.max(-85, Math.min(85, yaw + delta.yaw)),
    pitch: Math.max(-65, Math.min(15, pitch + delta.pitch)),
  };
}

export function voltageRows(status) {
  const observed = status?.voltage_v;
  const current = status?.current_ma;
  const requested = status?.requested_voltage_v;
  const zeroEvidence = status?.zero_evidence?.state === 'measured_zero'
    ? 'Host-observed zero' : 'Unknown';
  return Array.from({ length: 8 }, (_, index) => ({
    channel: index + 1,
    requested: finite(requested?.[index]) ? `${requested[index].toFixed(3)} V` : 'Unknown',
    observed: finite(observed?.[index]) ? `${observed[index].toFixed(3)} V` : 'Unknown',
    current: finite(current?.[index]) ? `${current[index].toFixed(3)} mA` : 'Unknown',
    zeroEvidence,
  }));
}

export function gainSummary(status) {
  const value = name => status?.fields?.[name]?.value;
  const temperature = finite(value('temperature_c')) ? `${value('temperature_c').toFixed(3)} °C` : 'Unknown';
  const current = finite(value('current_ma')) ? `${value('current_ma').toFixed(3)} mA` : 'Unknown';
  const switchValue = name => value(name) === true ? 'On' : value(name) === false ? 'Off' : 'Unknown';
  return {
    temperature,
    target: finite(value('target_c')) ? `${value('target_c').toFixed(3)} °C` : 'Unknown',
    current,
    tec: switchValue('tec_enabled'),
    output: switchValue('current_enabled'),
    interlock: 'Driver verification required',
  };
}
