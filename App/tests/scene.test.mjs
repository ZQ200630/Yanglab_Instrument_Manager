import assert from 'node:assert/strict';
import test from 'node:test';
import {describeStage, previewStageMove} from '../web/view-model.js';

const left = {side: 'left', serial: '2110148249-10', available: true,
  baseline_known: true, nominal_authorized: true,
  estimated_position_um: {x: .1, y: -.2, z: .3}};

test('stage text requires matching identity and valid authority before showing an estimate', () => {
  const initial = describeStage(left);
  assert.equal(initial.position, 'X 0.100 µm · Y -0.200 µm · Z 0.300 µm');
  assert.equal(initial.canMove, true);
  for (const change of [{serial: 'wrong'}, {side: 'unknown'}, {available: false},
    {baseline_known: false}, {nominal_authorized: false}, {fault: 'lost link'},
    {status_error: 'timeout'}, {restricted: true},
    {estimated_position_um: {x: NaN, y: 0, z: 0}}]) {
    const view = describeStage({...left, ...change});
    assert.equal(view.position, 'Unknown', JSON.stringify(change));
    assert.equal(view.canMove, false, JSON.stringify(change));
  }
});

test('stage draft stays in micrometers and preserves physical side limits without display scaling', () => {
  const draft = {x: .1, y: 0, z: 0};
  assert.equal(previewStageMove('left', draft).label, 'X +0.100 µm · Y +0.000 µm · Z +0.000 µm');
  assert.deepEqual(draft, {x: .1, y: 0, z: 0});
  for (const x of [.201, Infinity, NaN]) {
    assert.equal(previewStageMove('left', {x, y: 0, z: 0}).allowed, false);
  }
  assert.equal(previewStageMove('right', {x: -.201, y: 0, z: 0}).allowed, false);
  assert.equal(previewStageMove('left', {x: -1, y: 1, z: 1}).allowed, true);
});
