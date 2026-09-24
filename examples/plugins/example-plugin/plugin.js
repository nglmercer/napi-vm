import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export default class ExamplePlugin {
  onLoad(context) {
    this.config = JSON.parse(readFileSync("./config.json", "utf8"));
    this.banner = readFileSync("./assets/banner.txt", "utf8");

    writeFileSync(
      join("./cache", "status.json"),
      JSON.stringify({
        loaded: true,
        plugin: context.name,
        version: context.version,
        greeting: this.config.greeting,
      })
    );

    return this.config.greeting;
  }

  onUnload(context) {
    return { config: this.config, reason: context.reason };
  }

  onReload(context, previousState) {
    if (previousState && previousState.config) {
      this.config = previousState.config;
    } else {
      this.config = JSON.parse(readFileSync("./config.json", "utf8"));
    }
    writeFileSync(
      join("./cache", "status.json"),
      JSON.stringify({ reloaded: true, plugin: context.name })
    );
    return this.config.greeting;
  }
}
