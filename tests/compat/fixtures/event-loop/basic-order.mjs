const events = [];

events.push("sync-1");
Promise.resolve().then(() => events.push("promise"));
queueMicrotask(() => events.push("microtask"));
setTimeout(() => {
  events.push("timer");
  console.log(JSON.stringify(events));
}, 0);
events.push("sync-2");
