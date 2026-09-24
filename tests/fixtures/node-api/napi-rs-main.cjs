const addon = require("./fixture.node");

module.exports = {
  sum: addon.add(19, 23),
  text: addon.concatenate("rust", "-napi"),
};
