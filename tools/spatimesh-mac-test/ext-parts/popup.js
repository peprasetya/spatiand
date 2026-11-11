console.log("[ext-parts] popup opened");
let n = 0;
document.getElementById("b").onclick = () => { document.getElementById("t").textContent = "clicked " + (++n); console.log("[ext-parts] popup button clicked", n); };
chrome.runtime.sendMessage({ what: "tabs" }, a => { document.getElementById("t").textContent = a; });
