// ABI loader only. Application state, requests and rendering are implemented by the compiled module.
const preloadedHref = (rel, asset) =>
  document.head?.querySelector(`link[rel="${rel}"][href*="${asset}"]`)?.getAttribute("href");
const jsHref = preloadedHref("modulepreload", "/assets/graph_map_ui.js") || "/assets/graph_map_ui.js";
const wasmHref = preloadedHref("preload", "/assets/graph_map_ui_bg.wasm") || "/assets/graph_map_ui_bg.wasm";

try {
  const { default: init } = await import(new URL(jsHref, import.meta.url));
  await init({ module_or_path: new URL(wasmHref, import.meta.url) });
} catch (error) {
  console.error("Graph Map UI startup failed.", error);
  const note = document.getElementById("connecting");
  if (note) {
    note.textContent = "Could not load the graph-map interface. Run from localhost and refresh.";
    note.hidden = false;
  }
}
