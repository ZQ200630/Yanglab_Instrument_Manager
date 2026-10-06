import { previewStageMove } from './view-model.js';

const roles = new Set(['osa', 'voltage', 'gain', 'pm400', 'fiber']);

export function connectRequest(role, resource) {
  if (!roles.has(role)) throw new Error('Unknown instrument role');
  if (role === 'fiber') {
    if (resource != null) throw new Error('Fiber binding is fixed by serial number');
    return { role, resource: null, acknowledge_lifecycle: false };
  }
  if (typeof resource !== 'string' || !resource.trim()) {
    throw new Error(`${role} resource is required`);
  }
  return {
    role, resource: resource.trim(),
    acknowledge_lifecycle: role === 'osa' || role === 'voltage' || role === 'gain',
  };
}

export function fiberMoveRequest(side, displacement) {
  const preview = previewStageMove(side, displacement);
  if (!preview.allowed) throw new Error(preview.reason);
  return {
    role: 'fiber', name: 'move', side,
    dx: displacement.x, dy: displacement.y, dz: displacement.z,
  };
}

export function baselineConfirmations(side) {
  if (!['left', 'right'].includes(side)) throw new Error('Fiber side must be left or right');
  const label = side === 'left' ? 'left' : 'right';
  return [
    `Adopt the ${label} MDT voltage readbacks as the session position baseline. Verify the side and X/Y/Z voltages first. This does not move the stage or mean its physical displacement is zero.`,
    `Authorize the ${label} side to use the uncalibrated MAX312D nominal conversion (20 µm / 75 V). Displacement is an open-loop estimate; re-adopt the baseline after manual movement or abnormal state.`,
  ];
}

export function numericSetting(raw, minimum, maximum, label) {
  if (typeof raw !== 'string' || !raw.trim()) throw new Error(`${label} must be finite`);
  const value = Number(raw);
  if (!Number.isFinite(value)) throw new Error(`${label} must be finite`);
  if (value < minimum || value > maximum) {
    throw new Error(`${label} range is ${minimum}–${maximum}`);
  }
  return value;
}

export function cleanupWarning(report) {
  if (!report || !Array.isArray(report.steps) || !Array.isArray(report.unreleased)) {
    return 'Cleanup report missing; resource release and output state cannot be confirmed';
  }
  const issues = [];
  if (report.unreleased.length) issues.push(`Unreleased: ${report.unreleased.join(', ')}`);
  const failed = report.steps.filter((step) => step.ok !== true);
  if (failed.length) {
    issues.push(`Cleanup steps failed: ${failed.map((step) =>
      `${step.role}/${step.action}${step.error ? ` (${step.error})` : ''}`).join('; ')}`);
  }
  if (report.voltage_zero && report.voltage_zero.state !== 'measured_zero') {
    issues.push('Voltage zero not confirmed by telemetry');
  }
  return issues.length ? `${issues.join('; ')}. Independently verify physical instrument state` : null;
}

/** Rust stop errors include a JSON evidence object followed by optional stderr. */
export function shutdownError(cause) {
  const error = new Error(cause?.message || String(cause));
  if (cause?.shutdownEvidence) {
    error.shutdownEvidence = structuredClone(cause.shutdownEvidence);
    return error;
  }
  const marker = 'shutdown evidence: ';
  const start = error.message.indexOf(marker);
  if (start < 0) return error;
  const source = error.message.slice(start + marker.length);
  let depth = 0, quoted = false, escaped = false;
  for (let index = 0; index < source.length; index++) {
    const char = source[index];
    if (quoted) {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === '"') quoted = false;
    } else if (char === '"') quoted = true;
    else if (char === '{') depth++;
    else if (char === '}' && --depth === 0) {
      try { error.shutdownEvidence = JSON.parse(source.slice(0, index + 1)); } catch { /* retain raw error */ }
      break;
    }
  }
  return error;
}

export function markStatusUnknown(state) {
  // A lost reply cannot prove that the child or any physical output stopped.
  // Keep its handle and last-known identity, but revoke live control affordances.
  state.status = {
    mode: 'unknown', devices: {}, observed_at: null,
    last_cleanup: state.status?.last_cleanup || null,
  };
  state.mode = 'unknown';
}

export function workerSubtitle(state) {
  if (!state.client) return 'Waiting for worker';
  if (state.mode === 'unknown') return 'Connection status unknown';
  return `${Object.keys(state.status?.devices || {}).length} instrument types open`;
}
