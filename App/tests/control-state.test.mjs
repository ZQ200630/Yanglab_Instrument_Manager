import assert from 'node:assert/strict';
import test from 'node:test';
let api = {};
try { api = await import('../web/control-state.js'); } catch (error) {
  if (error.code !== 'ERR_MODULE_NOT_FOUND') throw error;
}
const context = { session_id: 's1', connection_id: 'c1', epoch: 3 };
const ready = { context, revision: 4, mode: 'READY', confirmed: true };

test('transit time is included in conservative field age', () => {
  assert.equal(typeof api.fieldAge, 'function');
  assert.equal(api.fieldAge({ observed_age_s: 1 }, 4000, 10000, 11000), 6);
  assert.equal(api.fieldAge({ observed_age_s: null }, 0, 0, 0), null);
  assert.equal(api.fieldAge({ observed_age_s: -1 }, -1, 10, 9), 0);
});
test('snapshots cannot restore an old identity epoch or revision', () => {
  assert.equal(typeof api.canApplySnapshot, 'function');
  assert.equal(api.canApplySnapshot(ready, { ...ready, revision: 5 }), true);
  for (const incoming of [ready, { ...ready, revision: 3 },
    { ...ready, revision: 5, context: { ...context, epoch: 2 } },
    { ...ready, revision: 5, context: { ...context, connection_id: 'old' } },
    { ...ready, revision: 5, context: { ...context, session_id: 'old' } }]) {
    assert.equal(api.canApplySnapshot(ready, incoming), false);
  }
});
test('normal authority requires confirmation and a ready idle role while safety remains reachable', () => {
  assert.equal(typeof api.canSendNormal, 'function');
  assert.equal(api.canSendNormal(ready), true);
  for (const restricted of [{ ...ready, normalPending: {} }, { ...ready, safetyPending: {} },
    { ...ready, confirmed: false }, { ...ready, hostRestricted: true },
    ...['STOP_HELD', 'UNKNOWN', 'CLOSING'].map(mode => ({ ...ready, mode }))]) {
    assert.equal(api.canSendNormal(restricted), false);
    assert.equal(api.canSendSafety(restricted), true);
  }
  assert.equal(api.canSendSafety({ ...ready, context: { ...context, connection_id: null } }), false);
});
