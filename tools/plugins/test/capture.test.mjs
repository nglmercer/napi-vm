import test from 'node:test';
import assert from 'node:assert/strict';
import { capture } from '../capture.mjs';

test('capture waits for both output pipes after a child exits', async () => {
  const result = await capture('node', ['-e', `
    process.stdout.write('stdout completion marker');
    process.stderr.write('x'.repeat(70000) + 'stderr completion marker', () => process.exit(0));
  `]);
  assert.equal(result.stdout, 'stdout completion marker');
  assert.ok(result.stderr.endsWith('stderr completion marker'));
  assert.ok(result.stderr.length <= 65536);
});

test('capture reports the exit code and complete trailing diagnostics', async () => {
  await assert.rejects(capture('node', ['-e', `
    process.stdout.write('stdout completion marker');
    process.stderr.write('x'.repeat(70000) + 'stderr completion marker', () => process.exit(9));
  `]), error => {
    assert.match(error.message, /exited 9/);
    assert.ok(error.message.includes('stdout completion marker'));
    assert.ok(error.message.includes('stderr completion marker'));
    return true;
  });
});
