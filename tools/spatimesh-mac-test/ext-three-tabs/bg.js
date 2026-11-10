// Three tabs playing at once: window A with two (one in the background), window B with one.
const log = (...a) => console.log("[multi-test]", ...a);
const page = f => "file:///tmp/smtest/media/play.html?f=" + f;
async function run() {
  const a = await chrome.windows.create({ url: page("h264-aac.mp4") });
  await chrome.tabs.create({ windowId: a.id, url: page("vp9-opus.webm"), active: false });
  const b = await chrome.windows.create({ url: page("hevc-aac.mp4") });
  log("window A", a.id, "window B", b.id);
}
chrome.runtime.onInstalled.addListener(run);
