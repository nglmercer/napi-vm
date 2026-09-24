const createMessage = require("plugin-message");

export default {
  onLoad(context) {
    const result = createMessage(context);
    require("node:fs").writeFileSync(
      "./cache/status.json",
      JSON.stringify(result),
    );
    return result;
  },

  onReload(context, previousState) {
    return { ...previousState, reloaded: true };
  },

  onUnload(context) {
    return { reason: context.reason };
  },
};
