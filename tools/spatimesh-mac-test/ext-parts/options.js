console.log("[ext-parts] options page opened");
const c = document.getElementById("c");
chrome.storage.sync.get({ setting: false }, v => { c.checked = v.setting; document.getElementById("o").textContent = JSON.stringify(v); });
c.onchange = () => chrome.storage.sync.set({ setting: c.checked }, () => console.log("[ext-parts] setting saved", c.checked));
