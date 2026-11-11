console.log("[ext-parts] content script ran in", location.href);
document.documentElement.setAttribute("data-ext-parts", "ran");
window.addEventListener("message", e => {
  if (e.source === window && e.data && e.data.extParts) {
    chrome.runtime.sendMessage({ what: e.data.extParts }, answer => {
      console.log("[ext-parts] answer:", answer);
      window.postMessage({ extPartsAnswer: answer }, "*");
    });
  }
});
