// What the rig asks of it comes as a tab's title: a page sets location.hash to #ext-<what>, the
// content script passes it on. Each part says in the console that it ran ([ext-parts] ...).
const log = (...a) => console.log("[ext-parts]", ...a);
chrome.runtime.onInstalled.addListener(async () => {
  await chrome.storage.sync.set({ installed: Date.now() });
  await chrome.storage.local.set({ local: "kept" });
  log("installed; storage.sync and storage.local written");
});
chrome.runtime.onMessage.addListener((m, sender, answer) => {
  log("asked", m.what, "by tab", sender.tab && sender.tab.id);
  (async () => {
    if (m.what === "options") { await chrome.runtime.openOptionsPage(); answer("options opened"); }
    else if (m.what === "popup") { try { await chrome.action.openPopup(); answer("popup opened"); } catch (e) { answer("popup: " + e.message); } }
    else if (m.what === "storage") { answer(JSON.stringify([await chrome.storage.sync.get(null), await chrome.storage.local.get(null)])); }
    else if (m.what === "tabs") { answer("tabs: " + (await chrome.tabs.query({})).length); }
    else answer("?");
  })();
  return true;
});
log("service worker started");
