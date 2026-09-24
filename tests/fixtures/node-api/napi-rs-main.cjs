const addon = require("./fixture.node");

const counter = new addon.Counter(40);
let failure;
try {
  addon.fail();
} catch (error) {
  failure = { name: error.name, message: error.message };
}

module.exports = (async () => ({
  sum: addon.add(19, 23),
  text: addon.concatenate("rust", "-napi"),
  counter: {
    initial: counter.value,
    incremented: counter.increment(),
    value: counter.value,
  },
  bytes: Array.from(addon.reverseBytes(Buffer.from([1, 2, 3, 4]))),
  failure,
  asyncSum: await addon.addAsync(20, 22),
}))();
