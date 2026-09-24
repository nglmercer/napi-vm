import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const native = require("rust-napi-plugin.node");

export default {
  async onLoad(context) {
    const config = JSON.parse(readFileSync("./config.json", "utf8"));
    const result = {
      plugin: context.name,
      greeting: native.greet(config.name),
      total: await native.addAsync(config.left, config.right),
    };
    writeFileSync(join("./cache", "result.json"), JSON.stringify(result));
    return result;
  },

  async onUnload(context) {
    return { reason: context.reason };
  },
};
