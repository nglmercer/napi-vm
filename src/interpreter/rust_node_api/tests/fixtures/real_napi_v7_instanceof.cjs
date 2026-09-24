
const addon = require('./fixture.node');
const calls = [];
class Marked {}
Object.defineProperty(Marked, Symbol.hasInstance, {
  configurable: true,
  value(value) {
    calls.push(this);
    return value && value.marked ? 'yes' : '';
  },
});
class MarkedChild extends Marked {}
const functionCalls = [];
function MarkedFunction() {}
Object.defineProperty(MarkedFunction, Symbol.hasInstance, {
  configurable: true,
  value(value) {
    functionCalls.push(this);
    return value && value.marked ? 'yes' : '';
  },
});
function InheritedMarkedFunction() {}
Object.setPrototypeOf(InheritedMarkedFunction, MarkedFunction);
const BoundMarkedFunction = MarkedFunction.bind(null);
const BoundOwnMarkedFunction = MarkedFunction.bind(null);
let boundOwnReceiverWasBound = false;
Object.defineProperty(BoundOwnMarkedFunction, Symbol.hasInstance, {
  configurable: true,
  value(value) {
    boundOwnReceiverWasBound = this === BoundOwnMarkedFunction;
    return value && value.own;
  },
});
function Ordinary(value) { this.value = value; }
const ordinary = new Ordinary(17);
const hasInstanceDescriptor = Object.getOwnPropertyDescriptor(
  Function.prototype,
  Symbol.hasInstance,
);
const BoundOrdinary = Ordinary.bind({value: -1}, 23);
const boundOrdinary = new BoundOrdinary();
class ClassConstructor { constructor(value) { this.value = value; } }
const BoundClassConstructor = ClassConstructor.bind(null, 31);
const boundClassInstance = new BoundClassConstructor();
function BoundAdd(left, right) { return this.base + left + right; }
const boundAdd = BoundAdd.bind({base: 10}, 4);
const reboundAdd = boundAdd.bind({base: 0}, 5);
const BoundArrow = (() => 1).bind(null);
let boundArrowConstructThrows = false;
try { new BoundArrow(); } catch (error) { boundArrowConstructThrows = error.name === 'TypeError'; }
function DeletedName() {}
const nameDeleted = delete DeletedName.name &&
  DeletedName.name === '' && !Object.hasOwn(DeletedName, 'name');
const ordinaryNames = Object.getOwnPropertyNames(Ordinary);
const ordinaryNameDescriptor = Object.getOwnPropertyDescriptor(Ordinary, 'name');
const ordinaryLengthDescriptor = Object.getOwnPropertyDescriptor(Ordinary, 'length');
const ordinaryPrototypeDescriptor = Object.getOwnPropertyDescriptor(Ordinary, 'prototype');
let proxyObjectGetPrototypeCalls = 0;
let napiProxyPrototypeTrapCalls = 0;
let readingNapiPrototype = false;
const napiReportedPrototype = {};
const proxyOrdinary = new Proxy(ordinary, {
  getPrototypeOf(target) {
    if (readingNapiPrototype) {
      napiProxyPrototypeTrapCalls++;
      return napiReportedPrototype;
    }
    proxyObjectGetPrototypeCalls++;
    return Reflect.getPrototypeOf(target);
  },
});
let proxyConstructorHasInstanceReads = 0;
const proxyOrdinaryConstructor = new Proxy(Ordinary, {
  get(target, key, receiver) {
    if (key === Symbol.hasInstance) proxyConstructorHasInstanceReads++;
    return Reflect.get(target, key, receiver);
  },
});
let proxyConstructorMissingHasInstanceReads = 0;
const proxyConstructorWithoutHasInstance = new Proxy(Ordinary, {
  get(target, key, receiver) {
    if (key === Symbol.hasInstance) {
      proxyConstructorMissingHasInstanceReads++;
      return undefined;
    }
    return Reflect.get(target, key, receiver);
  },
});
const nonExtensibleProxyTarget = Object.preventExtensions({});
const invalidGetPrototypeProxy = new Proxy(nonExtensibleProxyTarget, {
  getPrototypeOf() { return {}; },
});
let invalidGetPrototypeTrapThrows = false;
try {
  Object.getPrototypeOf(invalidGetPrototypeProxy);
} catch (error) {
  invalidGetPrototypeTrapThrows = error.name === 'TypeError';
}
readingNapiPrototype = true;
const napiProxyPrototype = addon.getPrototype(proxyOrdinary);
const napiTransparentProxyPrototype = addon.getPrototype(
  new Proxy(ordinary, {}),
);
readingNapiPrototype = false;
module.exports = {
  matched: addon.instanceofProbe({marked: true}, Marked),
  rejected: addon.instanceofProbe({marked: false}, Marked),
  inherited: addon.instanceofProbe({marked: true}, MarkedChild),
  guestMatched: ({marked: true}) instanceof Marked,
  guestInherited: ({marked: true}) instanceof MarkedChild,
  receiverWasConstructor: calls.length === 5 && calls[0] === Marked &&
    calls[1] === Marked && calls[2] === MarkedChild &&
    calls[3] === Marked && calls[4] === MarkedChild,
  functionMatched: addon.instanceofProbe({marked: true}, MarkedFunction),
  functionRejected: addon.instanceofProbe({marked: false}, MarkedFunction),
  functionInherited: addon.instanceofProbe({marked: true}, InheritedMarkedFunction),
  guestFunctionMatched: ({marked: true}) instanceof MarkedFunction,
  guestFunctionInherited: ({marked: true}) instanceof InheritedMarkedFunction,
  functionReceiverWasConstructor: functionCalls.length === 5 &&
    functionCalls[0] === MarkedFunction && functionCalls[1] === MarkedFunction &&
    functionCalls[2] === InheritedMarkedFunction &&
    functionCalls[3] === MarkedFunction &&
    functionCalls[4] === InheritedMarkedFunction,
  boundFunctionMatched: addon.instanceofProbe({marked: true}, BoundMarkedFunction),
  boundGuestFunctionMatched: ({marked: true}) instanceof BoundMarkedFunction,
  boundFunctionReceiverWasTarget: functionCalls.length === 7 &&
    functionCalls[5] === MarkedFunction && functionCalls[6] === MarkedFunction,
  boundOwnSymbolHasInstance: addon.instanceofProbe({own: true}, BoundOwnMarkedFunction),
  boundOwnGuestSymbolHasInstance: ({own: true}) instanceof BoundOwnMarkedFunction,
  boundOwnSymbolReceiverWasBound: boundOwnReceiverWasBound,
  ordinaryIsInstance: addon.instanceofProbe(ordinary, Ordinary),
  proxyObjectIsInstance: addon.instanceofProbe(proxyOrdinary, Ordinary),
  proxyObjectPrototypeMatches:
    Object.getPrototypeOf(proxyOrdinary) === Ordinary.prototype,
  napiProxyObjectPrototypeMatches:
    napiProxyPrototype === Ordinary.prototype,
  napiProxyObjectPrototypeMatchesTrapResult:
    napiProxyPrototype === napiReportedPrototype,
  napiProxyObjectPrototypeIsNull: napiProxyPrototype === null,
  napiTransparentProxyPrototypeIsNull: napiTransparentProxyPrototype === null,
  proxyObjectIsPrototypeOf: Ordinary.prototype.isPrototypeOf(proxyOrdinary),
  proxyConstructorIsInstance:
    addon.instanceofProbe(ordinary, proxyOrdinaryConstructor),
  proxyConstructorWithoutHasInstanceIsInstance:
    addon.instanceofProbe(ordinary, proxyConstructorWithoutHasInstance),
  invalidGetPrototypeTrapThrows,
  proxyTrapCalls: {
    objectGetPrototype: proxyObjectGetPrototypeCalls,
    napiGetPrototype: napiProxyPrototypeTrapCalls,
    constructorHasInstance: proxyConstructorHasInstanceReads,
    constructorMissingHasInstance: proxyConstructorMissingHasInstanceReads,
  },
  ordinaryGuestInstanceof: ordinary instanceof Ordinary,
  boundOrdinaryGuestInstanceof: boundOrdinary instanceof BoundOrdinary,
  boundOrdinaryTargetInstanceof: boundOrdinary instanceof Ordinary,
  boundOrdinaryNapiInstanceof: addon.instanceofProbe(boundOrdinary, BoundOrdinary),
  boundOrdinaryValue: boundOrdinary.value,
  boundOrdinaryHasNoOwnPrototype: !Object.hasOwn(BoundOrdinary, 'prototype'),
  boundClassGuestInstanceof: boundClassInstance instanceof BoundClassConstructor,
  boundClassTargetInstanceof: boundClassInstance instanceof ClassConstructor,
  boundClassNapiInstanceof: addon.instanceofProbe(boundClassInstance, BoundClassConstructor),
  boundClassValue: boundClassInstance.value,
  boundCallResult: boundAdd(2),
  reboundCallResult: reboundAdd(),
  boundName: boundAdd.name,
  boundLength: boundAdd.length,
  reboundName: reboundAdd.name,
  reboundLength: reboundAdd.length,
  boundArrowConstructThrows,
  functionPrototypeHasInstanceIsFunction:
    typeof Function.prototype[Symbol.hasInstance] === 'function',
  functionPrototypeHasInstanceDescriptor:
    hasInstanceDescriptor.writable === false &&
    hasInstanceDescriptor.enumerable === false &&
    hasInstanceDescriptor.configurable === false,
  functionPrototypeHasInstanceOrdinary:
    Function.prototype[Symbol.hasInstance].call(Ordinary, ordinary),
  functionPrototypeHasInstanceBound:
    Function.prototype[Symbol.hasInstance].call(BoundOrdinary, boundOrdinary),
  functionPrototypeHasInstanceRejectsNonFunction:
    Function.prototype[Symbol.hasInstance].call({}, ordinary) === false,
  ordinaryIsObject: ordinary instanceof Object,
  ordinaryIsFunction: Ordinary instanceof Function,
  ordinaryPrototypeIsObject: Ordinary.prototype instanceof Object,
  ordinaryPrototypeShared: Object.getPrototypeOf(ordinary) === Ordinary.prototype,
  ordinaryConstructorShared: ordinary.constructor === Ordinary,
  ordinaryDefaultFunctionPrototype: Object.getPrototypeOf(Ordinary) === Function.prototype,
  functionConstructorPrototype: Object.getPrototypeOf(Function) === Function.prototype,
  functionPrototypeIsCallable: typeof Function.prototype === 'function',
  functionPrototypeObjectPrototype: Object.getPrototypeOf(Ordinary.prototype) === Object.prototype,
  functionPrototypeConstructorShared: Function.prototype.constructor === Function,
  functionCallIsInherited: typeof Ordinary.call === 'function',
  functionApplyIsInherited: typeof Ordinary.apply === 'function',
  functionApplyUsesReceiverAndArguments: (function (a, b) {
    return this.base + a + b;
  }).apply({base: 3}, [4, 5]) === 12,
  functionApplyReadsArrayLike: (function (a, b) {
    return a + b;
  }).apply(null, {0: 'a', 1: 'b', length: 2}) === 'ab',
  functionPrototypeNapiIdentity: addon.getPrototype(Ordinary) === Function.prototype,
  functionPrototypeObjectNapiIdentity: addon.getPrototype(Ordinary.prototype) === Object.prototype,
  functionConstructorNapiIdentity: addon.getPrototype(Function) === Function.prototype,
  ordinaryName: Ordinary.name,
  ordinaryLength: Ordinary.length,
  ordinaryStandardProperties: ['length', 'name', 'prototype'].every(name =>
    ordinaryNames.includes(name)),
  ordinaryDescriptors: ordinaryNameDescriptor.writable === false &&
    ordinaryNameDescriptor.enumerable === false && ordinaryNameDescriptor.configurable === true &&
    ordinaryLengthDescriptor.value === 1 && ordinaryLengthDescriptor.writable === false &&
    ordinaryPrototypeDescriptor.writable === true &&
    ordinaryPrototypeDescriptor.enumerable === false &&
    ordinaryPrototypeDescriptor.configurable === false,
  nameDeleted,
  ordinaryValue: ordinary.value,
};
