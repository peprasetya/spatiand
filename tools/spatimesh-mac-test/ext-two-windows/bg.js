// Two windows with a near-silent tone each; after 12 s the second window's tab goes to the
// browser's first window (the oldest, not the newest), so a routed tab can be told from one
// heard at the newest window by default.
const log = (...a) => console.log("[sound-test]", ...a);
async function run() {
  const [first] = await chrome.windows.getAll();
  const w1 = await chrome.windows.create({ url: "file:///tmp/smtest/tone1.html" });
  const w2 = await chrome.windows.create({ url: "file:///tmp/smtest/tone2.html" });
  log("windows", first.id, w1.id, w2.id);
  setTimeout(async () => {
    const [t2] = await chrome.tabs.query({ windowId: w2.id });
    await chrome.tabs.move(t2.id, { windowId: first.id, index: -1 });
    await chrome.tabs.update(t2.id, { active: true });
    log("moved tab", t2.id, "to the first window", first.id);
  }, 12000);
}
chrome.runtime.onInstalled.addListener(run);
