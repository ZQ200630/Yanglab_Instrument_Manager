/** UI eligibility only. The worker and instrument drivers remain authoritative. */
export const roles = ['osa', 'voltage', 'gain', 'pm400', 'fiber'];
export const gainFields = ['temperature_c', 'target_c', 'tec_enabled', 'current_ma', 'current_enabled'];

export function sameConnection(a, b) {
  return Boolean(a?.session_id && b?.session_id && a.session_id === b.session_id &&
    a.connection_id === b.connection_id);
}

export function canApplySnapshot(current, incoming) {
  return sameConnection(current?.context, incoming?.context) &&
    Number.isSafeInteger(incoming.context.epoch) && incoming.context.epoch >= current.context.epoch &&
    Number.isSafeInteger(incoming.revision) && incoming.revision > current.revision;
}

export function fieldAge(field, roundTripMs, receivedAtMs, nowMs) {
  if (!Number.isFinite(field?.observed_age_s) ||
      ![roundTripMs, receivedAtMs, nowMs].every(Number.isFinite)) return null;
  return Math.max(0, field.observed_age_s) + Math.max(0, roundTripMs) / 1000 +
    Math.max(0, nowMs - receivedAtMs) / 1000;
}

export function canSendNormal(roleState) {
  return Boolean(roleState?.context?.session_id && roleState.confirmed &&
    roleState.mode === 'READY' && !roleState.normalPending && !roleState.safetyPending &&
    !roleState.activeRequest && !roleState.hostRestricted && !roleState.unknown && !roleState.stopHeld);
}

export function canSendSafety(roleState) {
  return Boolean(roleState?.context?.session_id && roleState.context.connection_id);
}

export function canResume(roleState) {
  return canSendSafety(roleState) && roleState.stopHeld &&
    roleState.safety?.state === 'STOP_HELD' && roleState.safetyEpoch === roleState.context.epoch &&
    !roleState.normalPending && !roleState.safetyPending && !roleState.activeRequest;
}

export function gainEvidence(device, name, nowMs = performance.now()) {
  const field = device?.fields?.[name];
  const timing = device?.timing;
  const age = timing ? fieldAge(field, timing.roundTripMs, timing.receivedAtMs, nowMs)
    : Number.isFinite(field?.observed_age_s) ? Math.max(0, field.observed_age_s) : null;
  const quality = field?.quality === 'error' ? 'error'
    : field?.value == null || age === null || !['fresh', 'stale'].includes(field?.quality) ? 'unknown'
      : field.quality === 'stale' || age > 5 ? 'stale' : 'fresh';
  return { ...field, age, quality };
}

export function canEnableCurrent(roleState, device, nowMs = performance.now()) {
  return canSendNormal(roleState) && device?.connected === true && !device.status_error &&
    gainFields.every(name => {
      const field = gainEvidence(device, name, nowMs);
      return field.quality === 'fresh' && field.connection_id === roleState.context.connection_id &&
        (name.endsWith('_enabled') ? typeof field.value === 'boolean' : Number.isFinite(field.value));
    }) && device.fields.tec_enabled.value === true;
}

export function safetyIntent(method, params) {
  if (method === 'disconnect') return 'disconnect';
  if (method !== 'action') return null;
  if (params.role === 'voltage' && params.name === 'zero') return 'zero';
  if (params.role === 'gain' && params.name === 'disable_current') return 'current_off';
  if (params.role === 'gain' && params.name === 'disable_tec') return 'tec_off';
  return null;
}

export function intentRank(intent) {
  return { zero: 1, current_off: 1, tec_off: 2, disconnect: 3 }[intent] || 0;
}
