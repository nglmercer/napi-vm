// Small normalized contract shared by the event-queue regressions and benchmark.
import { createHash } from 'node:crypto';
import { assertContract, canonical } from '@napi-vm/plugin-protocol';

const descriptor = {
  descriptorVersion: 1, id: 'example.queue', version: '1.0.0',
  methods: {}, errors: {}, events: { changed: { payload: '#/$defs/Payload' } },
};
const schemas = { $defs: { Payload: {
  type: 'object', properties: { text: { type: 'string' }, index: { type: 'integer', minimum: 0 } },
  required: ['text', 'index'], additionalProperties: false,
} } };
export const eventQueueContract = {
  descriptor, schemas,
  digest: createHash('sha256').update(canonical({ descriptor, schemas }) + '\n').digest('hex'),
};
assertContract(eventQueueContract);
