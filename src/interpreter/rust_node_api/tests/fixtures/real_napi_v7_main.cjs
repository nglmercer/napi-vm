
const addon = require('./fixture.node');
const values = addon.values;
const external = addon.external;
const externalProbe = addon.externalProbe();
const externalArrayBuffer = addon.makeExternalArrayBuffer();
const externalArrayBufferView = new Uint8Array(externalArrayBuffer);
externalArrayBufferView[2] = 77;
const externalArrayBufferAlias = addon.checkExternalArrayBuffer(externalArrayBuffer);
const detachedProbe = addon.arraybufferDetachmentProbe();
const errorName = callback => {
  try { callback(); return null; } catch (error) { return error.name; }
};
const arrayBufferDetachment = {
  ownedDetachStatus: detachedProbe.ownedDetachStatus,
  detachStatus: detachedProbe.detachStatus,
  secondDetachStatus: detachedProbe.secondDetachStatus,
  nonArrayBufferStatus: detachedProbe.nonArrayBufferStatus,
  detachedBefore: detachedProbe.detachedBefore,
  detachedAfter: detachedProbe.detachedAfter,
  detachedNonArrayBuffer: detachedProbe.detachedNonArrayBuffer,
  arraybufferLength: detachedProbe.arraybufferLength,
  viewLength: detachedProbe.viewLength,
  byteOffset: detachedProbe.byteOffset,
  dataViewLength: detachedProbe.dataViewLength,
  dataViewOffset: detachedProbe.dataViewOffset,
  guestArrayBufferLength: detachedProbe.buffer.byteLength,
  guestViewLength: detachedProbe.view.length,
  guestViewByteLength: detachedProbe.view.byteLength,
  guestViewByteOffset: detachedProbe.view.byteOffset,
  guestDataViewByteLength: errorName(() => detachedProbe.dataView.byteLength),
  guestDataViewByteOffset: errorName(() => detachedProbe.dataView.byteOffset),
  guestDataViewRead: errorName(() => detachedProbe.dataView.getUint8(0)),
  typedArrayConstruction: errorName(() => new Uint8Array(detachedProbe.buffer)),
  dataViewConstruction: errorName(() => new DataView(detachedProbe.buffer)),
  arrayBufferSlice: errorName(() => detachedProbe.buffer.slice(0)),
};
const externalBuffer = addon.makeExternalBuffer();
externalBuffer[1] = 88;
const externalBufferAlias = addon.checkExternalBuffer(externalBuffer);
const asyncContextEvents = [];
globalThis.asyncContextMicrotaskRan = false;
const asyncContextResult = addon.asyncContextProbe(function(value) {
  asyncContextEvents.push('callback');
  queueMicrotask(() => {
    asyncContextEvents.push('microtask');
    globalThis.asyncContextMicrotaskRan = true;
  });
  return `${this.label}:${value}`;
});
const asyncContextEventsAtReturn = asyncContextEvents.slice();
const externalType = typeof external;
const externalKeys = Object.keys(external);
const externalJson = JSON.stringify(external);
const definedMethod = addon.definedMethod();
const definedValueBefore = addon.definedValue;
addon.definedValue = 23;
const definedValueAfter = addon.definedValue;
const definedMethodDescriptor = Object.getOwnPropertyDescriptor(addon, 'definedMethod');
const definedValueDescriptor = Object.getOwnPropertyDescriptor(addon, 'definedValue');
const definedConstantDescriptor = Object.getOwnPropertyDescriptor(addon, 'definedConstant');
addon.targetProbe();
const targetCallCounts = addon.targetCounts();
new addon.targetProbe();
const targetConstructCounts = addon.targetCounts();
const identityError = new Error('same');
const strictEqual = {
  sameObject: addon.strictEqualProbe(values, values),
  distinctObjects: addon.strictEqualProbe({}, {}),
  equalNumbers: addon.strictEqualProbe(42, 42),
  nan: addon.strictEqualProbe(0 / 0, 0 / 0),
  sameError: addon.strictEqualProbe(identityError, identityError),
  distinctErrors: addon.strictEqualProbe(new Error('same'), new Error('same')),
};
const counter = new addon.Counter(40);
const counterNewTargetInfo = addon.counterNewTargetInfo();
const counterIncremented = counter.increment();
const counterStaticDescriptor = Object.getOwnPropertyDescriptor(addon.Counter, 'offset');
const counterStaticMethodDescriptor = Object.getOwnPropertyDescriptor(addon.Counter, 'constant');
const counterStaticBaseDescriptor = Object.getOwnPropertyDescriptor(addon.Counter, 'baseValue');
const counterStaticBefore = addon.Counter.offset;
addon.Counter.offset = 18;
const counterStaticAfter = addon.Counter.offset;
const counterReadOnlyBefore = addon.Counter.readOnly;
addon.Counter.readOnly = 99;
const counterReadOnlyAfter = addon.Counter.readOnly;
const counterStaticDeleteRejected = delete addon.Counter.offset;
Object.setPrototypeOf(addon.Counter, { inheritedStatic: 'inherited' });
class CounterChild extends addon.Counter {}
const counterInheritedStatic = CounterChild.inheritedStatic;
const counterHasInheritedStatic = 'inheritedStatic' in CounterChild;
const counterInheritedBaseValue = CounterChild.baseValue;
const counterInheritedStaticMethod = CounterChild.constant();
const childCounter = new CounterChild(5);
const childNewTargetInfo = addon.counterNewTargetInfo();
const instanceChecks = {
  counterIsCounter: addon.instanceofProbe(counter, addon.Counter),
  counterIsChild: addon.instanceofProbe(counter, CounterChild),
  childIsCounter: addon.instanceofProbe(childCounter, addon.Counter),
  childIsChild: addon.instanceofProbe(childCounter, CounterChild),
  numberIsCounter: addon.instanceofProbe(3, addon.Counter),
  typeErrorIsError: addon.instanceofProbe(new TypeError('fixture'), Error),
  typeErrorIsTypeError: addon.instanceofProbe(new TypeError('fixture'), TypeError),
};
const errors = addon.createErrors();
let typeError;
let rangeError;
let createdThrow;
try { addon.throwTypeError(); } catch (error) {
  typeError = {name: error.name, message: error.message, code: error.code,
    isTypeError: error instanceof TypeError, isError: error instanceof Error};
}
try { addon.throwRangeError(); } catch (error) {
  rangeError = {name: error.name, message: error.message,
    isRangeError: error instanceof RangeError, isError: error instanceof Error};
}
try { addon.throwCreatedError(); } catch (error) {
  createdThrow = {name: error.name, message: error.message,
    isTypeError: error instanceof TypeError, isError: error instanceof Error};
}
const cleared = addon.throwAndClear();
const wrapped = addon.wrapProbe();
const removedWrap = addon.removeWrapProbe();
const duplicateWrapStatus = addon.duplicateWrapStatus();
const reference = addon.referenceProbe();
const referenceReleased = addon.releaseReference();
const makeClosure = () => () => {};
const firstClosure = makeClosure();
const secondClosure = makeClosure();
const buffers = addon.bufferProbe();
const typedArrays = addon.typedArrayProbe();
const stringEncodings = addon.stringEncodingProbe('Aé€😀');
const dates = addon.dateProbe();
const bigintValues = addon.bigintProbe();
const bigintApi = {
  signed: bigintValues.signed.toString(),
  unsigned: bigintValues.unsigned.toString(),
  wide: bigintValues.wide.toString(),
  signedRoundtrip: bigintValues.signedRoundtrip.toString(),
  unsignedRoundtrip: bigintValues.unsignedRoundtrip.toString(),
  wrappedSigned: bigintValues.wrappedSigned.toString(),
  wrappedUnsigned: bigintValues.wrappedUnsigned.toString(),
  signedLossless: bigintValues.signedLossless,
  unsignedLossless: bigintValues.unsignedLossless,
  wrappedSignedLossless: bigintValues.wrappedSignedLossless,
  wrappedUnsignedLossless: bigintValues.wrappedUnsignedLossless,
  signBit: bigintValues.signBit,
  wordCount: bigintValues.wordCount,
  invalidTypeStatus: bigintValues.invalidTypeStatus,
};
const instanceDataMatches = addon.instanceDataProbe();
const dateConstructorAlias = Date;
const guestDate = new Date(17);
globalThis.Date = function ReplacementDate() {};
const aliasedDateInstance = guestDate instanceof dateConstructorAlias;
globalThis.Date = dateConstructorAlias;
const utf16 = addon.utf16Probe('Aé😀\0Z');
const elementDeleteTarget = [10, 20, 30];
const elementDelete = addon.deleteElementProbe(elementDeleteTarget, 1);
const escapableScope = addon.escapableScopeProbe();
const runScriptObservation = addon.runScriptProbe();
const runScriptResult = runScriptObservation.value;
const booleanCoercions = [undefined, null, false, 0, -0, NaN, '', 0n, [], {}]
  .map(value => addon.coerceToBoolean(value));
const numberCoercions = [undefined, null, false, true, '',
  ' ' + String.fromCharCode(0xFEFF) + ' ', '0x10', '0b11',
  '0o10', '1.5', 'Infinity', 'not a number'].map(value => addon.coerceToNumber(value));
const stringCoercions = [0, -0, null, undefined, true, 12n, [1, 2], {}]
  .map(value => addon.coerceToString(value));
const coercionEvents = [];
const guestNumberCoercion = addon.coerceToNumber({valueOf() {
  coercionEvents.push('number.valueOf');
  return '42';
}});
const guestStringCoercion = addon.coerceToString({toString() {
  coercionEvents.push('string.toString');
  return 23;
}});
const exoticCoercion = {[Symbol.toPrimitive](hint) {
  coercionEvents.push(`symbol:${hint}`);
  return hint === 'number' ? '44' : 'exotic';
}};
const exoticNumberCoercion = addon.coerceToNumber(exoticCoercion);
const exoticStringCoercion = addon.coerceToString(exoticCoercion);
function captureCoercionError(operation) {
  try { operation(); } catch (error) {
    return {name: error.name, isTypeError: error instanceof TypeError};
  }
}
const coercionErrors = {
  symbolNumber: captureCoercionError(() => addon.coerceToNumber(Symbol('value'))),
  symbolString: captureCoercionError(() => addon.coerceToString(Symbol('value'))),
  bigintNumber: captureCoercionError(() => addon.coerceToNumber(1n)),
};
const objectCoercions = [false, 12, 'abc', Symbol('value'), 13n].map(value => {
  const boxed = addon.coerceToObject(value);
  const expectedPrototype = typeof value === 'boolean' ? Boolean.prototype :
    typeof value === 'number' ? Number.prototype :
    typeof value === 'string' ? String.prototype :
    typeof value === 'symbol' ? Symbol.prototype : BigInt.prototype;
  return {type: typeof boxed, same: boxed === value,
    primitiveType: typeof boxed.valueOf(), string: boxed.toString(), length: boxed.length,
    guestPrototype: Object.getPrototypeOf(boxed) === expectedPrototype,
    napiPrototype: addon.getPrototype(boxed) === expectedPrototype};
});
const objectCoercionErrors = [null, undefined].map(value =>
  captureCoercionError(() => addon.coerceToObject(value)));
const objectCoercionPreservesIdentity = (() => {
  const object = {};
  return addon.coerceToObject(object) === object;
})();
let typedArrayError;
try { addon.invalidTypedArray(); } catch (error) {
  typedArrayError = {name: error.name, message: error.message, code: error.code,
    isRangeError: error instanceof RangeError, isError: error instanceof Error};
}
const callbackReceiver = {base: 40};
const callbackResult = addon.callGuest(function (amount) {
  this.base += addon.add(amount, 1);
  return this.base;
}, callbackReceiver, 1);
let callbackError;
try {
  addon.callGuest(function () { throw new RangeError('guest callback failure'); }, {}, 0);
} catch (error) {
  callbackError = {name: error.name, message: error.message,
    isRangeError: error instanceof RangeError, isError: error instanceof Error};
}
let callbackThrown;
try {
  addon.callGuest(function () { throw 'guest primitive failure'; }, {}, 0);
} catch (error) {
  callbackThrown = error;
}
class GuestBox {
  constructor(value) { this.value = value; }
}
const constructed = addon.constructGuest(GuestBox, 'constructed');
let propertyGetterCount = 0;
let propertySetterValue;
const propertyTarget = Object.create({inherited: true});
Object.defineProperty(propertyTarget, 'computed', {
  enumerable: true,
  configurable: true,
  get() { propertyGetterCount++; return 23; },
});
Object.defineProperty(propertyTarget, 'assigned', {
  enumerable: true,
  configurable: true,
  set(value) { propertySetterValue = value; },
});
propertyTarget.removeMe = true;
const properties = addon.propertyProbe(propertyTarget);
const propertyNamesTarget = Object.create({inheritedName: 'prototype', hidden: 'shadowed'});
Object.defineProperty(propertyNamesTarget, 'visible', {
  value: 'visible', enumerable: true, writable: true, configurable: true,
});
Object.defineProperty(propertyNamesTarget, 'hidden', {
  value: 'hidden', enumerable: false, writable: true, configurable: false,
});
propertyNamesTarget[3] = 'three';
propertyNamesTarget['01'] = 'named';
propertyNamesTarget[Symbol('own')] = 'symbol';
function reflectedFunction(argument) {}
const rawPropertyNames = addon.propertyNamesProbe(
  propertyNamesTarget, addon.Counter, reflectedFunction, Promise.resolve(1));
const describePropertyKey = key => typeof key === 'symbol'
  ? key.toString()
  : `${typeof key}:${key}`;
const propertyNames = {
  allOwn: rawPropertyNames.allOwn.map(describePropertyKey),
  enumerable: rawPropertyNames.enumerable.map(describePropertyKey),
  skipStrings: rawPropertyNames.skipStrings.map(describePropertyKey),
  withPrototype: rawPropertyNames.withPrototype.map(describePropertyKey),
  keepNumbers: rawPropertyNames.keepNumbers.map(describePropertyKey),
  writable: rawPropertyNames.writable.map(describePropertyKey),
  configurable: rawPropertyNames.configurable.map(describePropertyKey),
  // Node and Bun insert the class prototype in different positions; keep the
  // cross-runtime comparison focused on the shared class own-key set.
  class: rawPropertyNames.classNames.map(describePropertyKey).sort(),
  function: rawPropertyNames.functionNames.map(describePropertyKey).filter(name =>
    ['string:length', 'string:name', 'string:prototype', 'string:nativeProperty',
      'string:definedByNapi'].includes(name)),
  array: rawPropertyNames.arrayNames.map(describePropertyKey),
  promiseHasThen: rawPropertyNames.promiseHasThen,
  promiseHasCatch: rawPropertyNames.promiseHasCatch,
  promiseHasFinally: rawPropertyNames.promiseHasFinally,
};
const proxyPropertyNamesTarget = Object.create({proxyInherited: 'prototype'});
Object.defineProperty(proxyPropertyNamesTarget, 'fixed', {
  value: 'fixed', enumerable: false, writable: false, configurable: false,
});
proxyPropertyNamesTarget.visible = 'visible';
const proxyPropertyNamesSymbol = Symbol('proxy-own');
proxyPropertyNamesTarget[proxyPropertyNamesSymbol] = 'symbol';
let proxyOwnKeysCalls = 0;
const propertyNamesProxy = new Proxy(proxyPropertyNamesTarget, {
  ownKeys() {
    proxyOwnKeysCalls++;
    return ['visible', 'fixed', proxyPropertyNamesSymbol, 'virtual'];
  },
});
const rawProxyPropertyNames = addon.proxyPropertyNamesProbe(propertyNamesProxy);
const proxyPropertyNames = {
  allOwn: rawProxyPropertyNames.allOwn.map(describePropertyKey),
  enumerable: rawProxyPropertyNames.enumerable.map(describePropertyKey),
  skipStrings: rawProxyPropertyNames.skipStrings.map(describePropertyKey),
  withPrototype: rawProxyPropertyNames.withPrototype.map(describePropertyKey),
  writable: rawProxyPropertyNames.writable.map(describePropertyKey),
  configurable: rawProxyPropertyNames.configurable.map(describePropertyKey),
  ownKeysCalls: proxyOwnKeysCalls,
};
const globalPropertyNames = addon.globalPropertyNamesProbe();
const backingBytes = new Uint8Array(typedArrays.buffer);
const binaryTypedArray = new Uint8Array([1, 2, 3]);
const binaryBuffer = new ArrayBuffer(4);
const binaryDataView = new DataView(binaryBuffer);
const binarySharedBuffer = new SharedArrayBuffer(4);
const customPrototype = {marker: 'prototype'};
const customPrototypeTarget = Object.create(customPrototype);
const nullPrototypeTarget = Object.create(null);
module.exports = {
  same: addon === require('./fixture.node'),
  global: addon.globalProbe(),
  globalHasObject: Object.hasOwn(globalThis, 'Object'),
  definedMethod,
  definedValueBefore,
  definedValueAfter,
  targetCallCounts,
  targetConstructCounts,
  strictEqual,
  counterNewTargetInfo,
  childNewTargetInfo,
  childCounterValue: childCounter.value,
  instanceChecks,
  supportsNapiV7: addon.supportsNapiV7,
  bigintApi,
  arrayBufferDetachment,
  instanceDataMatches,
  propertyNames,
  proxyPropertyNames,
  globalPropertyNames,
  dateApi: {
    value: dates.value,
    guestValue: dates.date.getTime(),
    isDate: dates.isDate,
    guestPrototypeMatches: Object.getPrototypeOf(dates.date) === Date.prototype,
    nativeApiPrototypeMatches: dates.prototypeMatches,
    methodIsShared: dates.date.getTime === Date.prototype.getTime,
    prototypeParentMatches: Object.getPrototypeOf(Date.prototype) === Object.prototype,
    constructorMatches: Date.prototype.constructor === Date,
    methodsAreHidden: Object.keys(Date.prototype).length === 0,
    guestInstance: dates.date instanceof Date,
    guestConstructedInstance: guestDate instanceof Date,
    aliasedDateInstance,
    referenceMatches: dates.referenceMatches,
    napiInstance: dates.napiInstance,
    numberIsDate: dates.numberIsDate,
    invalidDateStatus: dates.invalidDateStatus,
  },
  definedConstant: addon.definedConstant,
  definedSymbolValue: addon[addon.descriptorSymbol],
  definedMethodEnumerable: definedMethodDescriptor.enumerable,
  definedValueEnumerable: definedValueDescriptor.enumerable,
  definedConstantWritable: definedConstantDescriptor.writable,
  counterValue: counter.value,
  counterIncremented,
  counterInstance: counter instanceof addon.Counter,
  counterConstructor: counter.constructor === addon.Counter,
  counterStaticMethod: addon.Counter.constant(),
  counterStaticBaseValue: addon.Counter.baseValue,
  counterStaticBefore,
  counterStaticAfter,
  counterReadOnlyBefore,
  counterReadOnlyAfter,
  counterStaticDeleteRejected,
  counterInheritedStatic,
  counterHasInheritedStatic,
  counterInheritedBaseValue,
  counterInheritedStaticMethod,
  externalProbe,
  externalArrayBufferAlias,
  externalBufferAlias,
  asyncContextResult,
  asyncContextEventsAtReturn,
  externalType,
  externalKeys,
  externalJson,
  counterStaticEnumerable: counterStaticDescriptor.enumerable,
  counterStaticMethodEnumerable: counterStaticMethodDescriptor.enumerable,
  counterStaticBaseWritable: counterStaticBaseDescriptor.writable,
  counterStaticKeys: Object.keys(addon.Counter).sort().join(','),
  symbols: addon.symbolProbe(),
  sum: addon.add(19, 23),
  version: addon.metadata.version,
  truth: values.truth,
  nothing: values.nothing === null,
  missing: values.missing === undefined,
  greeting: values.greeting,
  stringEncodings,
  utf16,
  elementDelete,
  escapableScope,
  runScriptResult,
  runScriptMicrotaskRanDuringCall: runScriptObservation.microtaskRanDuringCall,
  runScriptSideEffect: globalThis.napiRunScriptCount,
  elementDeleteLength: elementDeleteTarget.length,
  elementDeleteRemaining: [elementDeleteTarget[0], elementDeleteTarget[2]],
  elementDeleteHole: !(1 in elementDeleteTarget),
  booleanCoercions,
  numberCoercions,
  stringCoercions,
  guestNumberCoercion,
  guestStringCoercion,
  exoticNumberCoercion,
  exoticStringCoercion,
  coercionErrors,
  objectCoercions,
  objectCoercionErrors,
  objectCoercionPreservesIdentity,
  coercionEvents,
  fraction: values.fraction,
  maxUint32: values.maxUint32,
  int64: values.int64,
  roundTrip: addon.roundTrip(true, 4.25, 'native ✓', 4294967295, -2.5),
  int64Conversions: [NaN, Infinity, -Infinity, -0, 3.9, -3.9, 1e20, -1e20]
    .map(value => addon.int64ConversionProbe(value)),
  prototypes: {
    defaultMatches: addon.getPrototype({}) === Object.prototype,
    nativeFunctionMatches: addon.getPrototype(addon.add) === Function.prototype,
    customMatches: addon.getPrototype(customPrototypeTarget) === customPrototype,
    nullMatches: addon.getPrototype(nullPrototypeTarget) === null,
  },
  arrayPrototypes: {
    defaultMatches: Object.getPrototypeOf([]) === Array.prototype,
    prototypeParentMatches: Object.getPrototypeOf(Array.prototype) === Object.prototype,
    prototypeIsArray: Array.isArray(Array.prototype),
    constructorMatches: Array.prototype.constructor === Array,
    mapIsShared: Array.prototype.map === [].map,
    iteratorIsValues: Array.prototype[Symbol.iterator] === Array.prototype.values,
    mapDescriptor: (() => {
      const descriptor = Object.getOwnPropertyDescriptor(Array.prototype, 'map');
      return descriptor.writable && !descriptor.enumerable && descriptor.configurable;
    })(),
    keysAreEmpty: Object.keys(Array.prototype).length === 0,
    inheritsObjectMethod: typeof [].hasOwnProperty === 'function',
    nativeApiDefaultMatches: addon.getPrototype([]) === Array.prototype,
    nativeApiParentMatches: addon.getPrototype(Array.prototype) === Object.prototype,
    objectCreateInherits: Object.create(Array.prototype).map === Array.prototype.map,
    customPrototypeMutation: (() => {
      const prototype = { marker: true };
      const array = [1];
      Object.setPrototypeOf(array, prototype);
      return Object.getPrototypeOf(array) === prototype && array.marker &&
        array[0] === 1 && array.map === undefined;
    })(),
  },
  promisePrototypes: {
    defaultMatches: Object.getPrototypeOf(Promise.resolve(1)) === Promise.prototype,
    nativeApiMatches: addon.getPrototype(Promise.resolve(1)) === Promise.prototype,
    instanceofMatches: Promise.resolve(1) instanceof Promise,
    parentMatches: Object.getPrototypeOf(Promise.prototype) === Object.prototype,
    constructorMatches: Promise.prototype.constructor === Promise,
    methodIsShared: Promise.resolve(1).then === Promise.prototype.then,
    methodsAreHidden: Object.keys(Promise.prototype).length === 0,
  },
  binaryPrototypes: {
    typedArrayDefault: Object.getPrototypeOf(binaryTypedArray) === Uint8Array.prototype,
    typedArrayNapi: addon.getPrototype(binaryTypedArray) === Uint8Array.prototype,
    typedArrayParentsShared:
      Object.getPrototypeOf(Uint8Array.prototype) === Object.getPrototypeOf(Int8Array.prototype),
    typedArrayConstructor: Uint8Array.prototype.constructor === Uint8Array,
    typedArrayIteratorIsValues: Uint8Array.prototype[Symbol.iterator] ===
      Uint8Array.prototype.values,
    typedArrayMethodShared: binaryTypedArray.map === Uint8Array.prototype.map,
    typedArrayMethodCall: Array.from(Uint8Array.prototype.map.call(binaryTypedArray, x => x * 2))
      .join(',') === '2,4,6',
    typedArrayMethodsHidden: Object.keys(Uint8Array.prototype).length === 0,
    arrayBufferDefault: Object.getPrototypeOf(binaryBuffer) === ArrayBuffer.prototype,
    arrayBufferNapi: addon.getPrototype(binaryBuffer) === ArrayBuffer.prototype,
    arrayBufferMethodShared: binaryBuffer.slice === ArrayBuffer.prototype.slice,
    arrayBufferSlice: binaryBuffer.slice(1).byteLength === 3,
    sharedArrayBufferDefault:
      Object.getPrototypeOf(binarySharedBuffer) === SharedArrayBuffer.prototype,
    sharedArrayBufferNapi: addon.getPrototype(binarySharedBuffer) === SharedArrayBuffer.prototype,
    sharedArrayBufferMethodShared:
      binarySharedBuffer.slice === SharedArrayBuffer.prototype.slice,
    dataViewDefault: Object.getPrototypeOf(binaryDataView) === DataView.prototype,
    dataViewNapi: addon.getPrototype(binaryDataView) === DataView.prototype,
    dataViewMethodShared: binaryDataView.getUint8 === DataView.prototype.getUint8,
    dataViewMethodCall: DataView.prototype.setUint8.call(binaryDataView, 0, 9) === undefined &&
      binaryDataView.getUint8(0) === 9,
    dataViewMethodsHidden: Object.keys(DataView.prototype).length === 0,
    napiBufferIsBuffer: Buffer.isBuffer(buffers.copy),
    napiBufferDefault: Object.getPrototypeOf(buffers.copy) === Buffer.prototype,
    napiBufferPrototype: addon.getPrototype(buffers.copy) === Buffer.prototype,
    bufferPrototypeParent: Object.getPrototypeOf(Buffer.prototype) === Uint8Array.prototype,
    bufferConstructorParent: Object.getPrototypeOf(Buffer) === Uint8Array,
    napiBufferText: buffers.copy.toString('hex') === '41784344',
    napiBufferJson: JSON.stringify(buffers.copy) ===
      '{"type":"Buffer","data":[65,120,67,68]}',
  },
  array: addon.arrayProbe(),
  wrapped,
  removedWrap,
  duplicateWrapStatus,
  distinctFunctionIdentity: firstClosure !== secondClosure,
  buffers: {
    copy: [buffers.copy[0], buffers.copy[1], buffers.copy[2], buffers.copy[3]],
    allocated: [buffers.allocated[0], buffers.allocated[1], buffers.allocated[2]],
    copyIsBuffer: buffers.copyIsBuffer,
    allocatedIsBuffer: buffers.allocatedIsBuffer,
    arrayIsBuffer: buffers.arrayIsBuffer,
    copyLength: buffers.copyLength,
    allocatedLength: buffers.allocatedLength,
  },
  typedArrays: {
    bytes: [backingBytes[0], backingBytes[1], backingBytes[2], backingBytes[3],
      backingBytes[4], backingBytes[5], backingBytes[6], backingBytes[7]],
    isArrayBuffer: typedArrays.isArrayBuffer,
    isTypedArray: typedArrays.isTypedArray,
    isDataView: typedArrays.isDataView,
    kind: typedArrays.typedKind,
    typedLength: typedArrays.typedLength,
    typedOffset: typedArrays.typedOffset,
    viewLength: typedArrays.viewLength,
    viewOffset: typedArrays.viewOffset,
  },
  typedArrayError,
  callbackResult,
  callbackReceiverBase: callbackReceiver.base,
  callbackError,
  callbackThrown,
  constructedValue: constructed.value,
  properties: {
    computed: properties.computed,
    hasInherited: properties.hasInherited,
    hasNamedInherited: properties.hasNamedInherited,
    hasOwnInherited: properties.hasOwnInherited,
    deleted: properties.deleted,
    getterCount: propertyGetterCount,
    setterValue: propertySetterValue,
    removedFromGuest: !Object.hasOwn(propertyTarget, 'removeMe'),
    names: properties.names,
  },
  undefinedResult: addon.returnsUndefined() === undefined,
  errors: {
    error: {name: errors.error.name, message: errors.error.message,
      code: errors.error.code, isError: errors.error instanceof Error},
    typeError: {name: errors.typeError.name, message: errors.typeError.message,
      code: errors.typeError.code, isTypeError: errors.typeError instanceof TypeError,
      isError: errors.typeError instanceof Error},
    rangeError: {name: errors.rangeError.name, message: errors.rangeError.message,
      code: errors.rangeError.code, isRangeError: errors.rangeError instanceof RangeError,
      isError: errors.rangeError instanceof Error},
  },
  typeError,
  rangeError,
  createdThrow,
  cleared: {name: cleared.name, message: cleared.message, code: cleared.code},
  reference: {
    sameValue: reference.value === values,
    countAfterUnref: reference.countAfterUnref,
    countAfterRef: reference.countAfterRef,
    released: referenceReleased,
  },
};
