const addon = require("./fixture.node");

const counter = new addon.Counter(40);
let failure;
try {
  addon.fail();
} catch (error) {
  failure = { name: error.name, message: error.message };
}
const attempt = (operation) => {
  try {
    return { value: operation() };
  } catch (error) {
    return { error: error.message, name: error.name, code: error.code };
  }
};

module.exports = (async () => ({
  sum: addon.add(19, 23),
  text: addon.concatenate("rust", "-napi"),
  counter: {
    initial: counter.value,
    incremented: counter.increment(),
    value: counter.value,
  },
  bytes: Array.from(addon.reverseBytes(Buffer.from([1, 2, 3, 4]))),
  profile: attempt(() => addon.updateProfile({ name: "Ada", scores: [30, 37], active: true })),
  sumValues: attempt(() => addon.sumValues([1, 2, 3])),
  optionalValues: [
    attempt(() => addon.optionalLabel("ready")),
    attempt(() => addon.optionalLabel(null)),
    attempt(() => addon.optionalLabel()),
  ],
  callback: attempt(() => addon.applyCallback("hello", value => value.toUpperCase())),
  json: attempt(() => addon.jsonRoundTrip({ nested: [1, "two", null], enabled: true })),
  enumValues: [
    attempt(() => addon.echoMode("Fast")),
    attempt(() => addon.echoMode("Safe")),
    attempt(() => addon.echoMode("Unknown")),
  ],
  bigints: [
    attempt(() => addon.roundTripBigint(123456789012345678901234567890n).toString()),
    attempt(() => addon.roundTripBigint(-98765432109876543210987654321n).toString()),
  ],
  typedArray: attempt(() => Array.from(addon.reverseTypedArray(new Uint8Array([1, 2, 255])))),
  failure,
  asyncSum: await addon.addAsync(20, 22),
}))();
