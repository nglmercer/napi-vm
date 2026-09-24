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
  failure,
  asyncSum: await addon.addAsync(20, 22),
}))();
